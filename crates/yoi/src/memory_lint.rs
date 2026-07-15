use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use memory::linter::WriteMode;
use memory::{Linter, WorkspaceLayout};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintCliOptions {
    pub workspace: Option<PathBuf>,
    pub json: bool,
    pub warnings_as_errors: bool,
}

impl LintCliOptions {
    fn workspace_root(&self) -> Result<PathBuf, LintCliError> {
        let cwd = std::env::current_dir().map_err(LintCliError::CurrentDir)?;
        Ok(match &self.workspace {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => cwd.join(path),
            None => cwd,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageError {
    MissingValue(&'static str),
    UnknownArgument(String),
    UnexpectedArgument(String),
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingValue(flag) => write!(f, "{flag} requires a value"),
            Self::UnknownArgument(arg) => write!(f, "unknown memory lint argument: {arg}"),
            Self::UnexpectedArgument(arg) => write!(f, "unexpected memory lint argument: {arg}"),
        }
    }
}

pub fn parse_lint_args(args: &[String]) -> Result<LintCliOptions, UsageError> {
    let mut options = LintCliOptions {
        workspace: None,
        json: false,
        warnings_as_errors: false,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                options.json = true;
                i += 1;
            }
            "--warnings-as-errors" => {
                options.warnings_as_errors = true;
                i += 1;
            }
            "--workspace" => {
                let raw = args
                    .get(i + 1)
                    .ok_or(UsageError::MissingValue("--workspace"))?;
                if raw.starts_with('-') {
                    return Err(UsageError::MissingValue("--workspace"));
                }
                options.workspace = Some(PathBuf::from(raw));
                i += 2;
            }
            arg if arg.starts_with("--workspace=") => {
                let value = arg.trim_start_matches("--workspace=");
                if value.is_empty() {
                    return Err(UsageError::MissingValue("--workspace"));
                }
                options.workspace = Some(PathBuf::from(value));
                i += 1;
            }
            arg if arg.starts_with('-') => {
                return Err(UsageError::UnknownArgument(arg.to_string()));
            }
            arg => return Err(UsageError::UnexpectedArgument(arg.to_string())),
        }
    }

    Ok(options)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintStatus {
    Clean,
    Failed,
}

#[derive(Debug)]
pub enum LintCliError {
    CurrentDir(io::Error),
    Io { path: PathBuf, source: io::Error },
    Output(io::Error),
}

impl fmt::Display for LintCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentDir(source) => write!(f, "failed to resolve current directory: {source}"),
            Self::Io { path, source } => write!(f, "io error at {}: {source}", path.display()),
            Self::Output(source) => write!(f, "failed to write lint output: {source}"),
        }
    }
}

impl std::error::Error for LintCliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CurrentDir(source) | Self::Io { source, .. } | Self::Output(source) => {
                Some(source)
            }
        }
    }
}

pub fn run(options: &LintCliOptions) -> Result<LintStatus, LintCliError> {
    let stdout = io::stdout();
    run_with_writer(options, stdout.lock())
}

pub fn run_with_writer<W: Write>(
    options: &LintCliOptions,
    mut writer: W,
) -> Result<LintStatus, LintCliError> {
    let workspace = options.workspace_root()?;
    let report = lint_workspace(&workspace)?;

    if options.json {
        write_json_report(&mut writer, &report)?;
    } else {
        write_human_report(&mut writer, &report)?;
    }

    if report.counts.errors > 0 || (options.warnings_as_errors && report.counts.warnings > 0) {
        Ok(LintStatus::Failed)
    } else {
        Ok(LintStatus::Clean)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceLintReport {
    pub workspace: String,
    pub files: Vec<FileLintReport>,
    pub errors: Vec<LintDiagnostic>,
    pub warnings: Vec<LintDiagnostic>,
    pub counts: LintCounts,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileLintReport {
    pub path: String,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LintDiagnostic {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct LintCounts {
    pub files: usize,
    pub errors: usize,
    pub warnings: usize,
    pub failing_files: usize,
}

pub fn lint_workspace(workspace: &Path) -> Result<WorkspaceLintReport, LintCliError> {
    let layout = WorkspaceLayout::new(workspace.to_path_buf());
    let linter = Linter::new(layout.clone());
    let mut paths = collect_record_paths(&layout)?;
    paths.sort();

    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let content = std::fs::read_to_string(&path).map_err(|source| LintCliError::Io {
            path: path.clone(),
            source,
        })?;
        let lint = linter.lint(&path, &content, WriteMode::Update);
        files.push(FileLintReport {
            path: display_path(workspace, &path),
            errors: lint.errors.iter().map(ToString::to_string).collect(),
            warnings: lint.warnings.iter().map(ToString::to_string).collect(),
        });
    }

    let counts = count_files(&files);
    let errors = diagnostics_for(&files, DiagnosticKind::Error);
    let warnings = diagnostics_for(&files, DiagnosticKind::Warning);
    Ok(WorkspaceLintReport {
        workspace: workspace.display().to_string(),
        files,
        errors,
        warnings,
        counts,
    })
}

fn collect_record_paths(layout: &WorkspaceLayout) -> Result<Vec<PathBuf>, LintCliError> {
    let mut paths = Vec::new();

    let summary = layout.summary_path();
    if is_file(&summary)? {
        paths.push(summary);
    }

    collect_md_files(&layout.decisions_dir(), &mut paths)?;
    collect_md_files(&layout.requests_dir(), &mut paths)?;
    collect_md_files(&layout.knowledge_dir(), &mut paths)?;

    Ok(paths)
}

fn is_file(path: &Path) -> Result<bool, LintCliError> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(meta.is_file()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(LintCliError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn collect_md_files(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<(), LintCliError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(LintCliError::Io {
                path: dir.to_path_buf(),
                source,
            });
        }
    };

    for entry in entries {
        let entry = entry.map_err(|source| LintCliError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| LintCliError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            paths.push(path);
        }
    }

    Ok(())
}

fn diagnostics_for(files: &[FileLintReport], kind: DiagnosticKind) -> Vec<LintDiagnostic> {
    files
        .iter()
        .flat_map(|file| {
            let messages = match kind {
                DiagnosticKind::Error => &file.errors,
                DiagnosticKind::Warning => &file.warnings,
            };
            messages.iter().map(|message| LintDiagnostic {
                path: file.path.clone(),
                message: message.clone(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum DiagnosticKind {
    Error,
    Warning,
}

fn count_files(files: &[FileLintReport]) -> LintCounts {
    files.iter().fold(
        LintCounts {
            files: files.len(),
            ..LintCounts::default()
        },
        |mut counts, file| {
            counts.errors += file.errors.len();
            counts.warnings += file.warnings.len();
            if !file.errors.is_empty() {
                counts.failing_files += 1;
            }
            counts
        },
    )
}

fn display_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn write_human_report<W: Write>(
    writer: &mut W,
    report: &WorkspaceLintReport,
) -> Result<(), LintCliError> {
    writeln!(writer, "Workspace: {}", report.workspace).map_err(LintCliError::Output)?;
    for file in &report.files {
        let status = if file.errors.is_empty() && file.warnings.is_empty() {
            "ok"
        } else if file.errors.is_empty() {
            "warning"
        } else {
            "error"
        };
        writeln!(writer, "{status}: {}", file.path).map_err(LintCliError::Output)?;
        for error in &file.errors {
            writeln!(writer, "  error: {error}").map_err(LintCliError::Output)?;
        }
        for warning in &file.warnings {
            writeln!(writer, "  warning: {warning}").map_err(LintCliError::Output)?;
        }
    }
    writeln!(
        writer,
        "Summary: files={} errors={} warnings={} failing_files={}",
        report.counts.files,
        report.counts.errors,
        report.counts.warnings,
        report.counts.failing_files
    )
    .map_err(LintCliError::Output)
}

fn write_json_report<W: Write>(
    writer: &mut W,
    report: &WorkspaceLintReport,
) -> Result<(), LintCliError> {
    serde_json::to_writer_pretty(&mut *writer, report)
        .map_err(|source| LintCliError::Output(io::Error::other(source)))?;
    writeln!(writer).map_err(LintCliError::Output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn valid_summary() -> &'static str {
        "---\nupdated_at: 2026-05-31T00:00:00Z\n---\nsummary body\n"
    }

    fn valid_request() -> &'static str {
        "---\ncreated_at: 2026-05-31T00:00:00Z\nupdated_at: 2026-05-31T00:00:00Z\nsources: []\n---\nrequest body\n"
    }

    fn warning_request() -> String {
        format!(
            "---\ncreated_at: 2026-05-31T00:00:00Z\nupdated_at: 2026-05-31T00:00:00Z\nsources:\n  - segment_id: seg\n    range: [0, 1]\n---\n{}\n",
            "x".repeat(1500)
        )
    }

    #[test]
    fn parses_lint_options() {
        let args = vec![
            "--workspace".to_string(),
            "/tmp/ws".to_string(),
            "--json".to_string(),
            "--warnings-as-errors".to_string(),
        ];
        let parsed = parse_lint_args(&args).unwrap();
        assert_eq!(parsed.workspace, Some(PathBuf::from("/tmp/ws")));
        assert!(parsed.json);
        assert!(parsed.warnings_as_errors);
    }

    #[test]
    fn rejects_lint_usage_errors() {
        assert_eq!(
            parse_lint_args(&["--workspace".to_string()]).unwrap_err(),
            UsageError::MissingValue("--workspace")
        );
        assert_eq!(
            parse_lint_args(&["--workspace".to_string(), "--json".to_string()]).unwrap_err(),
            UsageError::MissingValue("--workspace")
        );
        assert_eq!(
            parse_lint_args(&["--bogus".to_string()]).unwrap_err(),
            UsageError::UnknownArgument("--bogus".to_string())
        );
        assert_eq!(
            parse_lint_args(&["extra".to_string()]).unwrap_err(),
            UsageError::UnexpectedArgument("extra".to_string())
        );
    }

    #[test]
    fn lints_only_workspace_memory_and_knowledge_records() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(&root.join(".yoi/memory/summary.md"), valid_summary());
        write(
            &root.join(".yoi/memory/requests/request-one.md"),
            valid_request(),
        );
        write(
            &root.join(".yoi/memory/_logs/ignored.md"),
            "not frontmatter",
        );

        let report = lint_workspace(root).unwrap();
        assert_eq!(
            report
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                ".yoi/memory/requests/request-one.md",
                ".yoi/memory/summary.md",
            ]
        );
        assert_eq!(report.counts.files, 2);
        assert_eq!(report.counts.errors, 0);
    }

    #[test]
    fn invalid_records_count_as_lint_failures() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(&root.join(".yoi/memory/summary.md"), "missing frontmatter");

        let report = lint_workspace(root).unwrap();
        assert_eq!(report.counts.files, 1);
        assert_eq!(report.counts.errors, 1);
        assert_eq!(report.counts.failing_files, 1);
        assert!(report.files[0].errors[0].contains("frontmatter"));
    }

    #[test]
    fn warnings_as_errors_changes_status_without_changing_report() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(
            &root.join(".yoi/memory/requests/large-record.md"),
            &warning_request(),
        );

        let mut output = Vec::new();
        let status = run_with_writer(
            &LintCliOptions {
                workspace: Some(root.to_path_buf()),
                json: false,
                warnings_as_errors: true,
            },
            &mut output,
        )
        .unwrap();

        assert_eq!(status, LintStatus::Failed);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("warnings=1"));
        assert!(text.contains("errors=0"));
    }

    #[test]
    fn json_output_is_machine_readable() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(&root.join(".yoi/memory/summary.md"), valid_summary());

        let mut output = Vec::new();
        let status = run_with_writer(
            &LintCliOptions {
                workspace: Some(root.to_path_buf()),
                json: true,
                warnings_as_errors: false,
            },
            &mut output,
        )
        .unwrap();

        assert_eq!(status, LintStatus::Clean);
        let parsed: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(parsed["workspace"], root.display().to_string());
        assert_eq!(parsed["counts"]["files"], 1);
        assert_eq!(parsed["files"][0]["path"], ".yoi/memory/summary.md");
        assert!(parsed["files"][0]["errors"].as_array().unwrap().is_empty());
    }
}
