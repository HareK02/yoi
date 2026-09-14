use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use manifest::plugin::{
    MaterializedPluginPackage, PluginDiagnostic, PluginDiagnosticKind, PluginDiagnosticPhase,
    PluginPackageLimits, PluginPermission, PluginSourceKind, PluginTemplateResource,
    RUST_COMPONENT_INSTANCE_TEMPLATE, RUST_COMPONENT_TOOL_TEMPLATE, SourceQualifiedPluginId,
    read_plugin_directory, read_plugin_package_file, write_plugin_package_file,
};
use serde::Serialize;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MAX_TEXT: usize = 240;

#[derive(Clone, Debug, Default)]
pub(crate) struct PluginCliArgs {
    pub json: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum PluginCliCommand {
    New {
        template: String,
        destination: PathBuf,
        args: PluginCliArgs,
    },
    Check {
        input: PathBuf,
        args: PluginCliArgs,
    },
    Pack {
        input: PathBuf,
        output: Option<PathBuf>,
        args: PluginCliArgs,
    },
}

pub(crate) fn run(command: PluginCliCommand) -> Result<()> {
    if let PluginCliCommand::Check { input, args } = command {
        let report = build_check_report(&input);
        let rendered = render_check_report(&report, &args)?;
        print!("{rendered}");
        if report.status == "rejected" {
            return Err("plugin check failed; see diagnostics above".into());
        }
        return Ok(());
    }
    let rendered = match command {
        PluginCliCommand::New {
            template,
            destination,
            args,
        } => render_new(&template, &destination, &args)?,
        PluginCliCommand::Check { .. } => unreachable!("handled above"),
        PluginCliCommand::Pack {
            input,
            output,
            args,
        } => render_pack(&input, output.as_deref(), &args)?,
    };
    print!("{rendered}");
    Ok(())
}

fn render_new(template: &str, destination: &Path, args: &PluginCliArgs) -> Result<String> {
    let (template_name, resources) = embedded_template_resources(template)?;
    materialize_template(destination, resources)?;
    let mut next_steps = vec![
        "Review plugin.toml and generated Rust source.".to_string(),
        "Replace the placeholder plugin.component.wasm with a real built component before offline package verification.".to_string(),
        "Run `yoi plugin check <path>` and then `yoi plugin pack <path>`.".to_string(),
    ];
    if template == "rust-component-service" {
        next_steps.insert(
            1,
            "Implement the proposed Service ingress output shape for offline package validation."
                .to_string(),
        );
    }
    let report = NewReport {
        command: "new",
        template: template_name,
        destination: destination.display().to_string(),
        files: resources
            .iter()
            .map(|resource| resource.path.to_string())
            .collect(),
        safety: AuthoringSafetyReport::default(),
        next_steps,
    };
    if args.json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(&report)?));
    }
    render_new_human(&report)
}

fn embedded_template_resources(
    template: &str,
) -> Result<(&'static str, &'static [PluginTemplateResource])> {
    match template {
        "rust-component-tool" => Ok(("rust-component-tool", RUST_COMPONENT_TOOL_TEMPLATE)),
        "rust-component-service" => Ok(("rust-component-service", RUST_COMPONENT_INSTANCE_TEMPLATE)),
        _ => Err(format!(
            "unsupported plugin template `{template}` (supported: rust-component-tool, rust-component-service)"
        )
        .into()),
    }
}

fn materialize_template(
    destination: &Path,
    resources: &'static [PluginTemplateResource],
) -> Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "plugin template destination `{}` is a symlink; refusing to follow it",
                    destination.display()
                )
                .into());
            }
            if !metadata.is_dir() {
                return Err(format!(
                    "plugin template destination `{}` already exists and is not a directory",
                    destination.display()
                )
                .into());
            }
            if fs::read_dir(destination)?.next().is_some() {
                return Err(format!(
                    "plugin template destination `{}` is not empty",
                    destination.display()
                )
                .into());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(destination)?;
        }
        Err(error) => return Err(error.into()),
    }

    for resource in resources {
        let relative = safe_template_relative_path(resource.path)?;
        let path = destination.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, resource.contents)?;
    }
    Ok(())
}

fn safe_template_relative_path(path: &str) -> Result<&Path> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("embedded plugin template path `{path}` is unsafe").into());
    }
    Ok(relative)
}

#[cfg(test)]
fn render_check(input: &Path, args: &PluginCliArgs) -> Result<String> {
    let report = build_check_report(input);
    render_check_report(&report, args)
}

fn render_check_report(report: &CheckReport, args: &PluginCliArgs) -> Result<String> {
    if args.json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(report)?));
    }
    render_check_human(report)
}

fn render_pack(input: &Path, output: Option<&Path>, args: &PluginCliArgs) -> Result<String> {
    let limits = PluginPackageLimits::default();
    let materialized = read_plugin_directory(input, PluginSourceKind::Explicit, &limits)
        .map_err(|diagnostic| plugin_diagnostic_error("plugin pack", diagnostic))?;
    let output_path = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_package_output_path(input));
    let packed = write_plugin_package_file(&materialized, &output_path, &limits)
        .map_err(|diagnostic| plugin_diagnostic_error("plugin pack", diagnostic))?;
    let report = PackReport {
        command: "pack",
        status: "packed",
        input_path: input.display().to_string(),
        output_path: packed.output_path.display().to_string(),
        package: PackageReport::from_materialized(&MaterializedPluginPackage {
            package: packed.package,
            files: materialized.files,
        }),
        safety: AuthoringSafetyReport::default(),
    };
    if args.json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(&report)?));
    }
    render_pack_human(&report)
}

fn default_package_output_path(input: &Path) -> PathBuf {
    let name = input
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("plugin");
    input.with_file_name(format!("{name}.yoi-plugin"))
}

fn build_check_report(input: &Path) -> CheckReport {
    let limits = PluginPackageLimits::default();
    let input_kind = if input.is_dir() {
        "directory"
    } else {
        "package"
    };
    let result = if input.is_dir() {
        read_plugin_directory(input, PluginSourceKind::Explicit, &limits)
    } else {
        read_plugin_package_file(input, PluginSourceKind::Explicit, &limits)
    };
    match result {
        Ok(materialized) => {
            let placeholder_diagnostic = placeholder_component_diagnostic(&materialized);
            let status = if placeholder_diagnostic.is_some() {
                "partial"
            } else {
                "verified"
            };
            let diagnostics = placeholder_diagnostic.into_iter().collect();
            CheckReport {
                command: "check",
                status,
                input_path: input.display().to_string(),
                input_kind,
                package: Some(PackageReport::from_materialized(&materialized)),
                diagnostics,
                safety: AuthoringSafetyReport::default(),
                next_steps: check_next_steps(status),
            }
        }
        Err(diagnostic) => CheckReport {
            command: "check",
            status: "rejected",
            input_path: input.display().to_string(),
            input_kind,
            package: None,
            diagnostics: vec![PluginDiagnosticReport::from_diagnostic(&diagnostic)],
            safety: AuthoringSafetyReport::default(),
            next_steps: vec![
                "Fix the reported package diagnostic and run `yoi plugin check` again.".to_string(),
            ],
        },
    }
}

fn placeholder_component_diagnostic(
    materialized: &MaterializedPluginPackage,
) -> Option<PluginDiagnosticReport> {
    let runtime = materialized.package.manifest.runtime.as_ref()?;
    let component = runtime.component.as_deref()?;
    let component_bytes = materialized.files.get(component)?;
    let placeholder_bytes = RUST_COMPONENT_TOOL_TEMPLATE
        .iter()
        .find(|resource| resource.path == "plugin.component.wasm")?
        .contents
        .as_bytes();
    if component_bytes != placeholder_bytes {
        return None;
    }
    Some(PluginDiagnosticReport {
        kind: "placeholder".to_string(),
        phase: "runtime".to_string(),
        message: format!(
            "plugin component runtime artifact `{component}` is the generated placeholder; replace it with a real built component before package verification"
        ),
    })
}

fn check_next_steps(status: &str) -> Vec<String> {
    match status {
        "verified" => vec![
            "Package metadata and archive contents are valid without executing Plugin code."
                .to_string(),
            "Keep this artifact for explicit offline inspection; Worker Plugin installation is not supported."
                .to_string(),
        ],
        "partial" => vec![
            "Replace the generated placeholder component artifact with a real built component."
                .to_string(),
            "Run `yoi plugin check <path>` again to verify the offline package.".to_string(),
        ],
        _ => vec!["Fix the reported diagnostics before packing this Plugin.".to_string()],
    }
}

fn plugin_diagnostic_error(context: &str, diagnostic: PluginDiagnostic) -> String {
    format!("{context} failed: {}", diagnostic.message)
}

fn render_new_human(report: &NewReport) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "created plugin template `{}` at {}",
        report.template, report.destination
    )?;
    writeln!(out, "files:")?;
    for file in &report.files {
        writeln!(out, "  - {file}")?;
    }
    writeln!(
        out,
        "safety: no network; embedded template only; no secrets generated"
    )?;
    writeln!(out, "next steps:")?;
    for step in &report.next_steps {
        writeln!(out, "  - {step}")?;
    }
    Ok(out)
}

fn render_check_human(report: &CheckReport) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "plugin check: {} [{}] input_kind={}",
        report.input_path, report.status, report.input_kind
    )?;
    if let Some(package) = &report.package {
        writeln!(
            out,
            "package: {} version={} digest={} entries={} source={} surfaces={} tools={}",
            package.reference,
            package.version,
            package.digest,
            package.entries.len(),
            package.source,
            join_or_none(&package.surfaces),
            package.tools.len()
        )?;
        writeln!(
            out,
            "installation: unsupported; this command only inspects the explicit input path"
        )?;
    }
    if report.diagnostics.is_empty() {
        writeln!(out, "diagnostics: none")?;
    } else {
        writeln!(out, "diagnostics:")?;
        for diagnostic in &report.diagnostics {
            writeln!(
                out,
                "  - kind={} phase={} message={}",
                diagnostic.kind, diagnostic.phase, diagnostic.message
            )?;
        }
    }
    writeln!(
        out,
        "safety: no Plugin execution; no enablement config mutation; no secrets generated"
    )?;
    writeln!(out, "next steps:")?;
    for step in &report.next_steps {
        writeln!(out, "  - {step}")?;
    }
    Ok(out)
}

fn render_pack_human(report: &PackReport) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "plugin pack: {} [{}]",
        report.output_path, report.status
    )?;
    writeln!(
        out,
        "package: {} version={} digest={} entries={}",
        report.package.reference,
        report.package.version,
        report.package.digest,
        report.package.entries.len()
    )?;
    writeln!(
        out,
        "safety: deterministic stored .yoi-plugin archive; no Plugin execution; no config mutation"
    )?;
    Ok(out)
}

#[derive(Serialize)]
struct AuthoringSafetyReport {
    no_network: bool,
    no_plugin_execution: bool,
    no_enablement_config_mutation: bool,
    no_secrets_generated: bool,
}

impl Default for AuthoringSafetyReport {
    fn default() -> Self {
        Self {
            no_network: true,
            no_plugin_execution: true,
            no_enablement_config_mutation: true,
            no_secrets_generated: true,
        }
    }
}

#[derive(Serialize)]
struct NewReport {
    command: &'static str,
    template: &'static str,
    destination: String,
    files: Vec<String>,
    safety: AuthoringSafetyReport,
    next_steps: Vec<String>,
}

#[derive(Serialize)]
struct CheckReport {
    command: &'static str,
    status: &'static str,
    input_path: String,
    input_kind: &'static str,
    package: Option<PackageReport>,
    diagnostics: Vec<PluginDiagnosticReport>,
    safety: AuthoringSafetyReport,
    next_steps: Vec<String>,
}

#[derive(Serialize)]
struct PackReport {
    command: &'static str,
    status: &'static str,
    input_path: String,
    output_path: String,
    package: PackageReport,
    safety: AuthoringSafetyReport,
}

#[derive(Serialize)]
struct PackageReport {
    reference: String,
    package: String,
    source: String,
    version: String,
    schema_version: u32,
    digest: String,
    input_path: String,
    entries: Vec<String>,
    surfaces: Vec<String>,
    tools: Vec<String>,
    permissions: Vec<String>,
}

impl PackageReport {
    fn from_materialized(materialized: &MaterializedPluginPackage) -> Self {
        Self {
            reference: package_reference(&materialized.package.identity),
            package: materialized.package.manifest.id.clone(),
            source: materialized.package.identity.source.to_string(),
            version: materialized.package.manifest.version.clone(),
            schema_version: materialized.package.manifest.schema_version,
            digest: materialized.package.digest.clone(),
            input_path: materialized.package.input_path.display().to_string(),
            entries: materialized.package.entries.iter().cloned().collect(),
            surfaces: materialized
                .package
                .manifest
                .surfaces
                .iter()
                .map(ToString::to_string)
                .collect(),
            tools: materialized
                .package
                .manifest
                .tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect(),
            permissions: materialized
                .package
                .manifest
                .permissions
                .iter()
                .map(|permission| permission_name(permission.clone()).to_string())
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct PluginDiagnosticReport {
    kind: String,
    phase: String,
    message: String,
}

impl PluginDiagnosticReport {
    fn from_diagnostic(diagnostic: &PluginDiagnostic) -> Self {
        Self {
            kind: diagnostic_kind(&diagnostic.kind).to_string(),
            phase: diagnostic_phase(&diagnostic.phase).to_string(),
            message: bound_text(diagnostic.message.clone()),
        }
    }
}

fn diagnostic_phase(phase: &PluginDiagnosticPhase) -> &'static str {
    match phase {
        PluginDiagnosticPhase::Inspection => "inspection",
        PluginDiagnosticPhase::Manifest => "manifest",
    }
}

fn package_reference(identity: &SourceQualifiedPluginId) -> String {
    identity.to_string()
}

fn permission_name(permission: PluginPermission) -> String {
    permission.label()
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        values.join(", ")
    }
}

fn diagnostic_kind(kind: &PluginDiagnosticKind) -> &'static str {
    match kind {
        PluginDiagnosticKind::Missing => "missing",
        PluginDiagnosticKind::Duplicate => "duplicate",
        PluginDiagnosticKind::Ambiguous => "ambiguous",
        PluginDiagnosticKind::Version => "version",
        PluginDiagnosticKind::Digest => "digest",
        PluginDiagnosticKind::Api => "api",
        PluginDiagnosticKind::Surface => "surface",
        PluginDiagnosticKind::Grant => "grant",
        PluginDiagnosticKind::Malformed => "malformed",
        PluginDiagnosticKind::Traversal => "traversal",
        PluginDiagnosticKind::Bounds => "bounds",
        PluginDiagnosticKind::Io => "io",
    }
}

fn bound_text(mut text: String) -> String {
    text = text.replace('\n', " ");
    if text.chars().count() > MAX_TEXT {
        text = text.chars().take(MAX_TEXT).collect::<String>();
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn explicit_template_check_and_pack_remain_offline_authoring_operations() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("example");
        let output = dir.path().join("example.yoi-plugin");
        let args = PluginCliArgs::default();

        let created = render_new("rust-component-tool", &source, &args).unwrap();
        assert!(created.contains("created plugin template"));

        let checked = render_check(&source, &args).unwrap();
        assert!(checked.contains("[partial]"));
        assert!(checked.contains("explicit input path"));

        let packed = render_pack(&source, Some(&output), &args).unwrap();
        assert!(packed.contains("[packed]"));
        assert!(output.is_file());
    }

    #[test]
    fn malformed_explicit_package_is_rejected_without_discovery() {
        let dir = tempdir().unwrap();
        let package = dir.path().join("broken.yoi-plugin");
        fs::write(&package, b"not a package").unwrap();

        let report = build_check_report(&package);
        assert_eq!(report.status, "rejected");
        assert!(report.package.is_none());
        assert_eq!(report.diagnostics.len(), 1);
    }

    #[test]
    fn cli_source_has_no_ambient_plugin_catalog_operations() {
        let source = include_str!("plugin_cli.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for forbidden in [
            "discover_plugins(",
            "resolve_enabled_plugins(",
            "PluginDiscoveryOptions",
            "ProfileResolver",
            ".yoi/plugins",
        ] {
            assert!(
                !source.contains(forbidden),
                "ambient Plugin authority returned through {forbidden}"
            );
        }
    }
}
