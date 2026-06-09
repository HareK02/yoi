use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCli {
    Help,
    Analyze(SessionAnalyzeOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAnalyzeOptions {
    pub path: PathBuf,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCliOutput {
    pub stdout: String,
    pub status: SessionCliStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCliStatus {
    Success,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCliError(String);

impl fmt::Display for SessionCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SessionCliError {}

pub fn parse_session_args(args: &[String]) -> Result<SessionCli, SessionCliError> {
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(SessionCli::Help);
    }
    match args[0].as_str() {
        "analyze" => parse_analyze_args(&args[1..]).map(SessionCli::Analyze),
        other => Err(SessionCliError(format!(
            "unknown yoi session command `{other}`"
        ))),
    }
}

fn parse_analyze_args(args: &[String]) -> Result<SessionAnalyzeOptions, SessionCliError> {
    let mut path = None;
    let mut json = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--" => {
                for positional in iter {
                    set_path(&mut path, positional)?;
                }
                break;
            }
            value if value.starts_with('-') => {
                return Err(SessionCliError(format!(
                    "unknown yoi session analyze option `{value}`"
                )));
            }
            positional => set_path(&mut path, positional)?,
        }
    }
    let path = path.ok_or_else(|| {
        SessionCliError("yoi session analyze requires an explicit session JSONL path".into())
    })?;
    if !json {
        return Err(SessionCliError(
            "initial yoi session analyze output requires --json".into(),
        ));
    }
    Ok(SessionAnalyzeOptions { path, json })
}

fn set_path(path: &mut Option<PathBuf>, value: &str) -> Result<(), SessionCliError> {
    if path.is_some() {
        return Err(SessionCliError(
            "yoi session analyze accepts exactly one path".into(),
        ));
    }
    *path = Some(PathBuf::from(value));
    Ok(())
}

pub fn run(cli: SessionCli) -> Result<SessionCliOutput, SessionCliError> {
    match cli {
        SessionCli::Help => Ok(SessionCliOutput {
            stdout: help_text().to_string(),
            status: SessionCliStatus::Success,
        }),
        SessionCli::Analyze(options) => {
            let report = session_analytics::analyze_session(&options.path)
                .map_err(|e| SessionCliError(e.to_string()))?;
            let stdout = serde_json::to_string_pretty(&report)
                .map_err(|e| SessionCliError(format!("failed to render JSON report: {e}")))?;
            Ok(SessionCliOutput {
                stdout: format!("{stdout}\n"),
                status: SessionCliStatus::Success,
            })
        }
    }
}

pub fn help_text() -> &'static str {
    "yoi session\n\nUsage:\n  yoi session analyze <SESSION_JSONL_PATH> --json\n\nOptions:\n      --json    Emit a machine-readable JSON analytics report\n  -h, --help    Print help\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_session_analyze_json() {
        let cli = parse_session_args(&[
            "analyze".to_string(),
            "/tmp/session.jsonl".to_string(),
            "--json".to_string(),
        ])
        .unwrap();
        assert_eq!(
            cli,
            SessionCli::Analyze(SessionAnalyzeOptions {
                path: PathBuf::from("/tmp/session.jsonl"),
                json: true,
            })
        );
    }

    #[test]
    fn run_session_analyze_outputs_json() {
        let mut fixture = tempfile::NamedTempFile::new().unwrap();
        let call = serde_json::json!({
            "kind":"assistant_item",
            "ts":1,
            "item":{
                "kind":"tool_call",
                "call_id":"r1",
                "name":"Read",
                "arguments":serde_json::json!({"file_path":"/tmp/a"}).to_string()
            }
        });
        writeln!(fixture, "{call}").unwrap();
        let output = run(SessionCli::Analyze(SessionAnalyzeOptions {
            path: fixture.path().to_path_buf(),
            json: true,
        }))
        .unwrap();
        assert_eq!(output.status, SessionCliStatus::Success);
        let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(value["tool_usage"]["total_tool_calls"], 1);
        assert_eq!(value["tool_usage"]["counts_by_tool"]["Read"], 1);
        assert_eq!(value["response_batches"]["total_responses"], 1);
        assert_eq!(value["response_batches"]["total_tool_calls"], 1);
        assert_eq!(
            value["response_batches"]["tools_per_response_histogram"][0]["tool_call_count"],
            1
        );
    }

    #[test]
    fn analyze_requires_json_for_initial_cli() {
        let err = parse_session_args(&["analyze".to_string(), "/tmp/session.jsonl".to_string()])
            .unwrap_err();
        assert!(err.to_string().contains("--json"));
    }
}
