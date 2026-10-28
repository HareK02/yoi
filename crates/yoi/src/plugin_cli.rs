use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use manifest::plugin::{
    InspectedPluginPackage, MaterializedPluginPackage, PluginConnectionSlot, PluginDiagnostic,
    PluginDiagnosticKind, PluginDiagnosticPhase, PluginPackageLimits, PluginRequestedGrant,
    PluginRuntimeManifest, PluginSourceKind, PluginTemplateResource,
    RUST_COMPONENT_HTTPS_TOOL_TEMPLATE, RUST_COMPONENT_TOOL_TEMPLATE, SourceQualifiedPluginId,
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
    Catalog {
        command: PluginCatalogCommand,
        args: PluginCliArgs,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum PluginCatalogCommand {
    Upload(PathBuf),
    Install(server_api::PluginInstallRequest),
    List(server_api::PluginListRequest),
    Show(String),
    AddArtifact {
        installation_id: String,
        request: server_api::PluginAddArtifactRequest,
    },
    Revoke {
        installation_id: String,
        request: server_api::PluginRevokeRequest,
    },
    Uninstall {
        installation_id: String,
        request: server_api::PluginUninstallRequest,
    },
    Operation(String),
}

pub(crate) fn parse_catalog_args(
    subcommand: &str,
    args: &[String],
) -> std::result::Result<PluginCliCommand, String> {
    let allowed: &[&str] = match subcommand {
        "upload" | "show" | "operation" => &[],
        "install" => &["--upload-id", "--alias", "--operation-key"],
        "list" => &["--cursor", "--limit"],
        "add-artifact" => &[
            "--upload-id",
            "--expected-last-operation-id",
            "--operation-key",
        ],
        "revoke" => &[
            "--artifact-digest",
            "--reason",
            "--expected-last-operation-id",
            "--operation-key",
        ],
        "uninstall" => &["--expected-last-operation-id", "--operation-key"],
        _ => return Err(format!("unknown yoi plugin subcommand `{subcommand}`")),
    };
    let mut options = std::collections::BTreeMap::new();
    let mut positional = Vec::new();
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--json" {
            json = true;
        } else if arg.starts_with('-') {
            let (name, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)));
            if !allowed.contains(&name) {
                return Err(format!("unknown yoi plugin {subcommand} option `{name}`"));
            }
            let value = match inline {
                Some(value) => value,
                None => {
                    index += 1;
                    args.get(index)
                        .map(String::as_str)
                        .ok_or_else(|| format!("{name} requires a value"))?
                }
            };
            if value.trim().is_empty() || value.starts_with('-') {
                return Err(format!("{name} requires a nonempty value"));
            }
            if options
                .insert(name.to_string(), value.to_string())
                .is_some()
            {
                return Err(format!("{name} must not be provided more than once"));
            }
        } else {
            positional.push(arg.clone());
        }
        index += 1;
    }
    let required = |name: &str| -> std::result::Result<String, String> {
        options
            .get(name)
            .cloned()
            .ok_or_else(|| format!("yoi plugin {subcommand} requires {name}"))
    };
    let one_positional = || -> std::result::Result<String, String> {
        match positional.as_slice() {
            [value] if !value.trim().is_empty() => Ok(value.clone()),
            _ => Err(format!(
                "yoi plugin {subcommand} requires exactly one {}",
                if subcommand == "upload" {
                    "package path"
                } else if subcommand == "operation" {
                    "operation ID"
                } else {
                    "installation ID"
                }
            )),
        }
    };
    if matches!(subcommand, "install" | "list") && !positional.is_empty() {
        return Err(format!(
            "yoi plugin {subcommand} accepts no positional arguments"
        ));
    }
    let command = match subcommand {
        "upload" => PluginCatalogCommand::Upload(PathBuf::from(one_positional()?)),
        "install" => PluginCatalogCommand::Install(server_api::PluginInstallRequest {
            upload_id: required("--upload-id")?,
            alias: required("--alias")?,
            operation_key: required("--operation-key")?,
        }),
        "list" => {
            let limit = options
                .get("--limit")
                .map(|value| {
                    value
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=100).contains(n))
                        .ok_or_else(|| "--limit must be between 1 and 100".to_string())
                })
                .transpose()?;
            PluginCatalogCommand::List(server_api::PluginListRequest {
                cursor: options.get("--cursor").cloned(),
                limit,
            })
        }
        "show" => PluginCatalogCommand::Show(one_positional()?),
        "add-artifact" => PluginCatalogCommand::AddArtifact {
            installation_id: one_positional()?,
            request: server_api::PluginAddArtifactRequest {
                upload_id: required("--upload-id")?,
                expected_last_operation: required("--expected-last-operation-id")?,
                operation_key: required("--operation-key")?,
            },
        },
        "revoke" => PluginCatalogCommand::Revoke {
            installation_id: one_positional()?,
            request: server_api::PluginRevokeRequest {
                artifact_digest: required("--artifact-digest")?,
                reason: required("--reason")?,
                expected_last_operation: required("--expected-last-operation-id")?,
                operation_key: required("--operation-key")?,
            },
        },
        "uninstall" => PluginCatalogCommand::Uninstall {
            installation_id: one_positional()?,
            request: server_api::PluginUninstallRequest {
                expected_last_operation: required("--expected-last-operation-id")?,
                operation_key: required("--operation-key")?,
            },
        },
        "operation" => PluginCatalogCommand::Operation(one_positional()?),
        _ => unreachable!("validated subcommand"),
    };
    Ok(PluginCliCommand::Catalog {
        command,
        args: PluginCliArgs { json },
    })
}

// Use the shared credential resolver without exposing token storage or adding a
// second login path. The generated client owns path encoding and response bounds.
struct PluginAuthorizer(client::BackendApiClient);

impl server_api::client_support::RequestAuthorizer for PluginAuthorizer {
    fn authorize(
        &self,
        request: server_api::client_support::AuthorizerRequest<'_>,
    ) -> std::result::Result<
        reqwest::header::HeaderMap,
        server_api::client_support::AuthorizationError,
    > {
        let prepared = self
            .0
            .request(request.method.clone(), request.path_and_query)
            .and_then(|builder| builder.build().map_err(client::BackendApiClientError::Http))
            .map_err(|_| server_api::client_support::AuthorizationError::new())?;
        Ok(prepared.headers().clone())
    }
}

#[derive(Debug, Serialize)]
struct CatalogFailure {
    status: Option<u16>,
    error: String,
    message: String,
    diagnostics: Vec<server_api::Diagnostic>,
}

impl CatalogFailure {
    fn local(error: impl std::fmt::Display) -> Self {
        Self {
            status: None,
            error: "plugin_cli".into(),
            message: error.to_string(),
            diagnostics: Vec::new(),
        }
    }
}

impl From<server_api::client_support::ClientError<server_api::RepositoryApiError>>
    for CatalogFailure
{
    fn from(
        error: server_api::client_support::ClientError<server_api::RepositoryApiError>,
    ) -> Self {
        match error {
            server_api::client_support::ClientError::Public { status, error } => Self {
                status: Some(status.as_u16()),
                error: error.error,
                message: error.message,
                diagnostics: error.diagnostics,
            },
            server_api::client_support::ClientError::Failure(error) => Self::local(error),
        }
    }
}

pub(crate) struct CatalogOutput {
    pub success: bool,
    pub stdout: String,
}

pub(crate) async fn run_catalog(
    command: PluginCatalogCommand,
    args: PluginCliArgs,
    target: client::ResolvedTarget,
) -> CatalogOutput {
    let result = async {
        let client::ResolvedTarget::Backend { base_url, workspace_id } = target else {
            return Err(CatalogFailure::local("Plugin catalog requires a Backend Workspace target; use --backend and --workspace-id"));
        };
        let backend = client::BackendApiClient::from_stored_token(&base_url).map_err(CatalogFailure::local)?;
        // No redirect may forward a bearer credential to another origin.
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
            .build().map_err(CatalogFailure::local)?;
        let api = server_api::ServerApiClient::builder(backend.origin().as_str())
            .map_err(CatalogFailure::local)?.client(http).authorizer(PluginAuthorizer(backend))
            .response_body_limit(16 * 1024 * 1024).build().map_err(CatalogFailure::local)?;
        execute_catalog(command, &api, workspace_id).await
    }.await;
    catalog_output(result, args.json)
}

fn catalog_output(
    result: std::result::Result<serde_json::Value, CatalogFailure>,
    json: bool,
) -> CatalogOutput {
    let (success, value) = match result {
        Ok(value) => (true, value),
        Err(error) => (
            false,
            serde_json::json!({
                "status": "failed", "diagnostic": error,
                "reconciliation": "A failed response does not prove a mutation was not committed. Reconcile the saved operation; retry only with the same operation key and exact input. Do not refresh expected-last-operation-id for a replay."
            }),
        ),
    };
    let rendered = serde_json::to_string_pretty(&value).expect("JSON value is serializable");
    let stdout = if json {
        format!("{rendered}\n")
    } else {
        format!(
            "plugin catalog: {}\n{rendered}\n",
            if success {
                "backend response (registration does not enable or start a Worker)"
            } else {
                "failed"
            }
        )
    };
    CatalogOutput { success, stdout }
}

async fn execute_catalog<A: server_api::client_support::RequestAuthorizer>(
    command: PluginCatalogCommand,
    api: &server_api::ServerApiClient<A>,
    workspace_id: String,
) -> std::result::Result<serde_json::Value, CatalogFailure> {
    let value = match command {
        PluginCatalogCommand::Upload(input) => {
            let bytes = read_upload(&input).map_err(CatalogFailure::local)?;
            serde_json::to_value(api.plugin_upload(workspace_id, bytes.into()).await?)
        }
        PluginCatalogCommand::Install(request) => {
            serde_json::to_value(api.plugin_install(workspace_id, request).await?)
        }
        PluginCatalogCommand::List(query) => {
            serde_json::to_value(api.plugin_list(workspace_id, query).await?)
        }
        PluginCatalogCommand::Show(id) => {
            serde_json::to_value(api.plugin_show(workspace_id, id).await?)
        }
        PluginCatalogCommand::AddArtifact {
            installation_id,
            request,
        } => serde_json::to_value(
            api.plugin_artifact_add(workspace_id, installation_id, request)
                .await?,
        ),
        PluginCatalogCommand::Revoke {
            installation_id,
            request,
        } => serde_json::to_value(
            api.plugin_revoke(workspace_id, installation_id, request)
                .await?,
        ),
        PluginCatalogCommand::Uninstall {
            installation_id,
            request,
        } => serde_json::to_value(
            api.plugin_uninstall(workspace_id, installation_id, request)
                .await?,
        ),
        PluginCatalogCommand::Operation(id) => {
            serde_json::to_value(api.plugin_operation(workspace_id, id).await?)
        }
    };
    value.map_err(CatalogFailure::local)
}

fn read_upload(input: &Path) -> Result<Vec<u8>> {
    let limit = PluginPackageLimits::default().max_package_size_bytes;
    let file = fs::File::open(input)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(
            "Plugin upload requires an explicit .yoi-plugin regular file; pack directories first"
                .into(),
        );
    }
    if metadata.len() > limit {
        return Err(
            format!("Plugin upload exceeds the shared package limit of {limit} bytes").into(),
        );
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(
            format!("Plugin upload exceeds the shared package limit of {limit} bytes").into(),
        );
    }
    Ok(bytes)
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
        PluginCliCommand::Catalog { .. } => {
            return Err("Plugin catalog requires Backend dispatch".into());
        }
    };
    print!("{rendered}");
    Ok(())
}

fn render_new(template: &str, destination: &Path, args: &PluginCliArgs) -> Result<String> {
    let (template_name, resources) = embedded_template_resources(template)?;
    materialize_template(destination, resources)?;
    let core_name = match template_name {
        "rust-component-https-tool" => "yoi_rust_component_https_template",
        _ => "yoi_rust_component_tool_template",
    };
    let next_steps = vec![
        "Review plugin.toml and generated Rust source. In Cargo.toml, replace the repository-relative yoi-plugin-pdk dependency path with an absolute local crates/plugin-pdk path or a pinned Yoi Git commit.".to_string(),
        "In the generated directory, install the wasm32-unknown-unknown Rust target and wasm-tools, then run `cargo build --release --target wasm32-unknown-unknown --target-dir ../guest-target`. Keep build output outside the package input directory so check/pack do not include build files.".to_string(),
        format!("Run `wasm-tools component new ../guest-target/wasm32-unknown-unknown/release/{core_name}.wasm -o plugin.component.wasm` to create the actual Component; cargo check alone does not build it."),
        "Run `yoi plugin check <path>` and then `yoi plugin pack <path>`; static validation does not establish Host implementation or execution support.".to_string(),
    ];
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
        "rust-component-https-tool" => Ok((
            "rust-component-https-tool",
            RUST_COMPONENT_HTTPS_TOOL_TEMPLATE,
        )),
        _ => Err(format!(
            "unsupported plugin template `{template}` (supported: rust-component-tool, rust-component-https-tool); Service and other surfaces are not supported"
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
        package: PackageReport::from_package(
            &packed.package,
            &packed.artifact_digest,
            packed.artifact_size,
        ),
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
        Ok(materialized) => CheckReport {
            command: "check",
            status: "verified",
            input_path: input.display().to_string(),
            input_kind,
            package: Some(PackageReport::from_materialized(&materialized)),
            diagnostics: Vec::new(),
            safety: AuthoringSafetyReport::default(),
            next_steps: vec![
                "Package schemas, actual Component imports/exports, and canonical archive metadata are statically valid; no Plugin code was executed.".to_string(),
                "Static validation does not establish Host implementation, HTTP availability, installation, or Worker execution support.".to_string(),
            ],
        },
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
            "package: {} version={} package_digest={} entries={} source={} surfaces={} tools={}",
            package.reference,
            package.version,
            package.package_digest,
            package.entries.len(),
            package.source,
            join_or_none(&package.surfaces),
            package.tools.len()
        )?;
        render_artifact_human(&mut out, package)?;
        writeln!(
            out,
            "scope: static validation of the explicit input path only; no Host implementation or execution support is established"
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
        "package: {} version={} package_digest={} entries={}",
        report.package.reference,
        report.package.version,
        report.package.package_digest,
        report.package.entries.len()
    )?;
    render_artifact_human(&mut out, &report.package)?;
    writeln!(
        out,
        "safety: deterministic stored .yoi-plugin archive; no Plugin execution; no config mutation"
    )?;
    Ok(out)
}

fn render_artifact_human(out: &mut String, package: &PackageReport) -> Result<()> {
    writeln!(
        out,
        "canonical archive: artifact_digest={} artifact_size={} bytes",
        package.artifact_digest, package.artifact_size
    )?;
    writeln!(out, "component world: {}", package.component.world)?;
    writeln!(
        out,
        "component imports: {}",
        join_or_none(&package.component.imports)
    )?;
    writeln!(
        out,
        "component exports: {}",
        join_or_none(&package.component.exports)
    )?;
    Ok(())
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
    package_digest: String,
    artifact_digest: String,
    artifact_size: u64,
    input_path: String,
    entries: Vec<String>,
    surfaces: Vec<String>,
    tools: Vec<String>,
    runtime: PluginRuntimeManifest,
    component: ComponentReport,
    connections: Vec<PluginConnectionSlot>,
    requested_grants: Vec<PluginRequestedGrant>,
}

impl PackageReport {
    fn from_materialized(materialized: &MaterializedPluginPackage) -> Self {
        Self::from_package(
            &materialized.package,
            &materialized.artifact_digest,
            materialized.artifact_size,
        )
    }

    fn from_package(
        package: &InspectedPluginPackage,
        artifact_digest: &str,
        artifact_size: u64,
    ) -> Self {
        Self {
            reference: package_reference(&package.identity),
            package: package.manifest.id.clone(),
            source: package.identity.source.to_string(),
            version: package.manifest.version.clone(),
            schema_version: package.manifest.schema_version,
            package_digest: package.package_digest.clone(),
            artifact_digest: artifact_digest.to_string(),
            artifact_size,
            input_path: package.input_path.display().to_string(),
            entries: package.entries.iter().cloned().collect(),
            surfaces: package
                .manifest
                .surfaces
                .iter()
                .map(ToString::to_string)
                .collect(),
            tools: package
                .manifest
                .tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect(),
            runtime: package.manifest.runtime.clone(),
            component: ComponentReport {
                world: package.component.world.clone(),
                imports: package.component.imports.clone(),
                exports: package.component.exports.clone(),
            },
            connections: package.manifest.connections.clone(),
            requested_grants: package.manifest.requested_grants.clone(),
        }
    }
}

#[derive(Serialize)]
struct ComponentReport {
    world: String,
    imports: Vec<String>,
    exports: Vec<String>,
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

    fn catalog_command(subcommand: &str, args: &[&str]) -> PluginCatalogCommand {
        let args = args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        match parse_catalog_args(subcommand, &args).unwrap() {
            PluginCliCommand::Catalog { command, .. } => command,
            _ => panic!("expected catalog command"),
        }
    }

    #[test]
    fn catalog_parser_requires_explicit_mutation_keys_and_observed_operations() {
        for (subcommand, args, missing) in [
            (
                "install",
                vec!["--upload-id", "upload", "--alias", "alias"],
                "--operation-key",
            ),
            (
                "add-artifact",
                vec![
                    "installation",
                    "--upload-id",
                    "upload",
                    "--operation-key",
                    "key",
                ],
                "--expected-last-operation-id",
            ),
            (
                "revoke",
                vec![
                    "installation",
                    "--artifact-digest",
                    "sha256:artifact",
                    "--reason",
                    "reason",
                    "--operation-key",
                    "key",
                ],
                "--expected-last-operation-id",
            ),
            (
                "uninstall",
                vec!["installation", "--operation-key", "key"],
                "--expected-last-operation-id",
            ),
            (
                "uninstall",
                vec!["installation", "--expected-last-operation-id", "observed"],
                "--operation-key",
            ),
        ] {
            let args = args.into_iter().map(str::to_string).collect::<Vec<_>>();
            assert!(
                parse_catalog_args(subcommand, &args)
                    .unwrap_err()
                    .contains(missing)
            );
        }
        for args in [
            vec![
                "installation",
                "--operation-key=",
                "--expected-last-operation-id=observed",
            ],
            vec![
                "installation",
                "--operation-key=key",
                "--operation-key=other",
                "--expected-last-operation-id=observed",
            ],
            vec![
                "installation",
                "--operation-key=key",
                "--expected-last-operation-id= ",
            ],
            vec![
                "installation",
                "--operation-key=key",
                "--expected-last-operation-id=observed",
                "--operator",
            ],
        ] {
            let args = args.into_iter().map(str::to_string).collect::<Vec<_>>();
            assert!(parse_catalog_args("uninstall", &args).is_err());
        }
        for limit in ["0", "101", "-1", "not-a-number"] {
            assert!(parse_catalog_args("list", &[format!("--limit={limit}")]).is_err());
        }
        assert!(parse_catalog_args("list", &["extra".into()]).is_err());
        assert!(parse_catalog_args("show", &[]).is_err());
        assert!(parse_catalog_args("operation", &["one".into(), "two".into()]).is_err());
    }

    #[test]
    fn upload_reads_explicit_regular_bytes_with_the_shared_package_bound() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("package.yoi-plugin");
        let bytes = b"exact uploaded bytes\0";
        fs::write(&input, bytes).unwrap();
        assert_eq!(read_upload(&input).unwrap(), bytes);
        assert!(
            read_upload(dir.path())
                .unwrap_err()
                .to_string()
                .contains("regular file")
        );
        let file = fs::File::create(&input).unwrap();
        file.set_len(PluginPackageLimits::default().max_package_size_bytes + 1)
            .unwrap();
        assert!(
            read_upload(&input)
                .unwrap_err()
                .to_string()
                .contains("shared package limit")
        );
    }

    // The real argv parser, generated HTTP client and output conversion are connected
    // here. Only the remote Server and credential issuance are replaced; this does
    // not attest Server authentication, catalog persistence or domain validation.
    mod catalog_http {
        use super::*;
        use std::time::Duration;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        struct TestAuthorizer;
        impl server_api::client_support::RequestAuthorizer for TestAuthorizer {
            fn authorize(
                &self,
                _: server_api::client_support::AuthorizerRequest<'_>,
            ) -> std::result::Result<
                reqwest::header::HeaderMap,
                server_api::client_support::AuthorizationError,
            > {
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(
                    reqwest::header::AUTHORIZATION,
                    "Bearer test-only".parse().unwrap(),
                );
                Ok(headers)
            }
        }

        async fn server(
            status: u16,
            response: serde_json::Value,
        ) -> (String, tokio::task::JoinHandle<(String, Vec<u8>)>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let task = tokio::spawn(async move {
                tokio::time::timeout(Duration::from_secs(10), async move {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    let (headers, body_start, content_len) = loop {
                        let mut buf = [0; 4096];
                        let len = stream.read(&mut buf).await.unwrap();
                        assert_ne!(len, 0, "request ended before headers");
                        request.extend_from_slice(&buf[..len]);
                        if let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n") {
                            let headers = String::from_utf8(request[..end].to_vec()).unwrap();
                            let content_len = headers.lines().find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                            }).unwrap_or(0);
                            break (headers, end + 4, content_len);
                        }
                    };
                    while request.len() - body_start < content_len {
                        let mut buf = [0; 4096];
                        let len = stream.read(&mut buf).await.unwrap();
                        assert_ne!(len, 0, "request ended before body");
                        request.extend_from_slice(&buf[..len]);
                    }
                    let body = serde_json::to_vec(&response).unwrap();
                    stream.write_all(format!("HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
                    stream.write_all(&body).await.unwrap();
                    (headers, request[body_start..].to_vec())
                }).await.expect("bounded mock Server request")
            });
            (origin, task)
        }

        fn installation() -> serde_json::Value {
            serde_json::json!({"installation_id":"inst/one", "alias":"alias", "package_id":"example", "last_operation":"saved-operation", "uninstalled":false, "artifacts":[]})
        }

        #[tokio::test]
        async fn catalog_commands_send_exact_workspace_paths_query_and_caller_mutation_inputs() {
            let dir = tempdir().unwrap();
            let package = dir.path().join("explicit.yoi-plugin");
            fs::write(&package, b"exact\0archive bytes").unwrap();
            let upload_path = package.to_str().unwrap();
            let operation = |kind| serde_json::json!({"operation_id":"saved-operation", "kind":kind, "installation":installation()});
            let mut summary = installation();
            summary.as_object_mut().unwrap().remove("artifacts");
            let upload = serde_json::json!({"upload_id":"upload", "expires_at":"2026-10-30T00:00:00Z", "artifact":{
                "package_id":"example", "name":"Example", "version":"1.0.0", "description":null,
                "package_digest":"sha256:logical", "artifact_digest":"sha256:archive", "artifact_size":19,
                "schema_version":2, "world":"yoi:plugin/tool-plugin@2.0.0", "required_imports":[], "tools":[],
                "connections":[], "requested_grants":[], "config_schema":{"identity":"example", "version":"1", "digest":"sha256:schema", "namespace":"example", "dcdl":"schema"},
                "revoked":false, "revocation_reason":null
            }});
            let cases = vec![
                (
                    "upload",
                    vec![upload_path],
                    "POST",
                    "/api/w/ws%20space/plugin-uploads",
                    None,
                    upload,
                ),
                (
                    "install",
                    vec![
                        "--upload-id=upload",
                        "--alias=alias",
                        "--operation-key=exact-key",
                    ],
                    "POST",
                    "/api/w/ws%20space/plugins",
                    Some(
                        serde_json::json!({"upload_id":"upload", "alias":"alias", "operation_key":"exact-key"}),
                    ),
                    operation("install"),
                ),
                (
                    "list",
                    vec!["--cursor=next /+=", "--limit=7"],
                    "GET",
                    "/api/w/ws%20space/plugins",
                    None,
                    serde_json::json!({"installations":[summary], "next_cursor":"opaque-next"}),
                ),
                (
                    "show",
                    vec!["inst/one"],
                    "GET",
                    "/api/w/ws%20space/plugins/inst%2Fone",
                    None,
                    installation(),
                ),
                (
                    "add-artifact",
                    vec![
                        "inst/one",
                        "--upload-id=upload",
                        "--expected-last-operation-id=observed-operation",
                        "--operation-key=exact-key",
                    ],
                    "POST",
                    "/api/w/ws%20space/plugins/inst%2Fone/artifacts",
                    Some(
                        serde_json::json!({"upload_id":"upload", "expected_last_operation":"observed-operation", "operation_key":"exact-key"}),
                    ),
                    operation("add_artifact"),
                ),
                (
                    "revoke",
                    vec![
                        "inst/one",
                        "--artifact-digest=sha256:archive",
                        "--reason=explicit reason",
                        "--expected-last-operation-id=observed-operation",
                        "--operation-key=exact-key",
                    ],
                    "POST",
                    "/api/w/ws%20space/plugins/inst%2Fone/revoke",
                    Some(
                        serde_json::json!({"artifact_digest":"sha256:archive", "reason":"explicit reason", "expected_last_operation":"observed-operation", "operation_key":"exact-key"}),
                    ),
                    operation("revoke"),
                ),
                (
                    "uninstall",
                    vec![
                        "inst/one",
                        "--expected-last-operation-id=observed-operation",
                        "--operation-key=exact-key",
                    ],
                    "POST",
                    "/api/w/ws%20space/plugins/inst%2Fone/uninstall",
                    Some(
                        serde_json::json!({"expected_last_operation":"observed-operation", "operation_key":"exact-key"}),
                    ),
                    operation("uninstall"),
                ),
                (
                    "operation",
                    vec!["saved/operation"],
                    "GET",
                    "/api/w/ws%20space/plugin-operations/saved%2Foperation",
                    None,
                    operation("install"),
                ),
            ];
            for (subcommand, args, method, path, expected_body, response) in cases {
                let (origin, captured) = server(200, response.clone()).await;
                let api = server_api::ServerApiClient::builder(origin.as_str())
                    .unwrap()
                    .authorizer(TestAuthorizer)
                    .build()
                    .unwrap();
                let result =
                    execute_catalog(catalog_command(subcommand, &args), &api, "ws space".into())
                        .await
                        .unwrap();
                assert_eq!(
                    result, response,
                    "{subcommand} output must retain authoritative metadata"
                );
                let output = catalog_output(Ok(result), true);
                assert!(output.success);
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&output.stdout).unwrap(),
                    response
                );
                let (headers, body) = captured.await.unwrap();
                let mut start = headers.lines().next().unwrap().split_whitespace();
                assert_eq!(start.next(), Some(method));
                let url =
                    reqwest::Url::parse(&format!("{origin}{}", start.next().unwrap())).unwrap();
                assert_eq!(url.path(), path, "{subcommand}");
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("authorization: bearer test-only")
                );
                if subcommand == "list" {
                    let query = url
                        .query_pairs()
                        .collect::<std::collections::BTreeMap<_, _>>();
                    assert_eq!(query.get("cursor").unwrap(), "next /+=");
                    assert_eq!(query.get("limit").unwrap(), "7");
                }
                if subcommand == "upload" {
                    assert_eq!(body, b"exact\0archive bytes");
                    assert!(headers.contains("application/octet-stream"));
                } else if let Some(expected) = expected_body {
                    assert_eq!(
                        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                        expected
                    );
                } else {
                    assert!(body.is_empty());
                }
            }
        }

        #[tokio::test]
        async fn catalog_rejections_keep_concrete_diagnostics_without_retry() {
            for (status, code) in [
                (401, "unauthenticated"),
                (403, "forbidden"),
                (409, "plugin_operation_conflict"),
                (413, "plugin_upload_too_large"),
            ] {
                let diagnostics = serde_json::json!([{"code":"catalog_rejected", "severity":"error", "message":"safe blocker detail"}]);
                let response = serde_json::json!({"error":code,"message":"concrete rejection detail","diagnostics":diagnostics});
                let (origin, captured) = server(status, response).await;
                let api = server_api::ServerApiClient::builder(origin.as_str())
                    .unwrap()
                    .authorizer(TestAuthorizer)
                    .build()
                    .unwrap();
                let result = execute_catalog(
                    catalog_command(
                        "uninstall",
                        &[
                            "inst",
                            "--expected-last-operation-id=observed",
                            "--operation-key=exact",
                        ],
                    ),
                    &api,
                    "ws".into(),
                )
                .await;
                let output = catalog_output(result, true);
                assert!(!output.success);
                let report: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
                assert_eq!(report["diagnostic"]["status"], status);
                assert_eq!(report["diagnostic"]["error"], code);
                assert_eq!(report["diagnostic"]["message"], "concrete rejection detail");
                assert_eq!(report["diagnostic"]["diagnostics"], diagnostics);
                assert!(
                    report["reconciliation"]
                        .as_str()
                        .unwrap()
                        .contains("same operation key and exact input")
                );
                let (_, body) = captured.await.unwrap();
                let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(body["expected_last_operation"], "observed");
                assert_eq!(body["operation_key"], "exact");
            }
        }
    }

    #[tokio::test]
    async fn catalog_standalone_target_is_rejected_with_json_diagnostic() {
        let output = run_catalog(
            catalog_command("list", &[]),
            PluginCliArgs { json: true },
            client::ResolvedTarget::Standalone,
        )
        .await;
        assert!(!output.success);
        let report: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
        assert!(
            report["diagnostic"]["message"]
                .as_str()
                .unwrap()
                .contains("Backend Workspace target")
        );
    }

    fn built_tool_package(destination: &Path) {
        render_new(
            "rust-component-tool",
            destination,
            &PluginCliArgs::default(),
        )
        .unwrap();
        fs::write(
            destination.join("plugin.component.wasm"),
            include_bytes!("../../plugin-pdk/tests/fixtures/tool-v2.component.wasm"),
        )
        .unwrap();
    }

    #[test]
    fn tool_templates_explain_actual_component_build_without_shipping_a_placeholder() {
        let dir = tempdir().unwrap();
        for template in ["rust-component-tool", "rust-component-https-tool"] {
            let destination = dir.path().join(template);
            let rendered =
                render_new(template, &destination, &PluginCliArgs { json: true }).unwrap();
            let report: serde_json::Value = serde_json::from_str(&rendered).unwrap();
            assert_eq!(report["template"], template);
            assert!(destination.join("plugin.toml").is_file());
            assert!(destination.join("src/lib.rs").is_file());
            assert!(!destination.join("plugin.component.wasm").exists());
            let steps = report["next_steps"].to_string();
            assert!(steps.contains("yoi-plugin-pdk"));
            assert!(steps.contains(
                "cargo build --release --target wasm32-unknown-unknown --target-dir ../guest-target"
            ));
            assert!(steps.contains("wasm-tools component new"));
            let cargo: toml::Value =
                toml::from_str(&fs::read_to_string(destination.join("Cargo.toml")).unwrap())
                    .unwrap();
            let core_name = cargo["package"]["name"].as_str().unwrap().replace('-', "_");
            assert!(steps.contains(&format!(
                "wasm-tools component new ../guest-target/wasm32-unknown-unknown/release/{core_name}.wasm -o plugin.component.wasm"
            )));
            assert!(steps.contains("does not establish Host implementation"));
            assert_eq!(report["safety"]["no_plugin_execution"], true);
        }
    }

    #[test]
    fn unsupported_templates_are_rejected_before_creating_a_destination() {
        let dir = tempdir().unwrap();
        let destination = dir.path().join("unsupported");
        for template in [
            "rust-component-service",
            "rust-component-instance",
            "unknown",
        ] {
            let error = render_new(template, &destination, &PluginCliArgs::default()).unwrap_err();
            assert!(error.to_string().contains("unsupported plugin template"));
            assert!(!destination.exists());
        }
    }

    #[test]
    fn unbuilt_and_noncomponent_templates_are_rejected_not_partially_verified_or_packed() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("unbuilt");
        let output = dir.path().join("rejected.yoi-plugin");
        let args = PluginCliArgs::default();
        render_new("rust-component-tool", &source, &args).unwrap();
        let missing = build_check_report(&source);
        assert_eq!(missing.status, "rejected");
        assert!(missing.package.is_none());
        assert!(!missing.diagnostics.is_empty());
        assert!(render_pack(&source, Some(&output), &args).is_err());
        assert!(!output.exists());

        for bytes in [b"placeholder".as_slice(), b"\0asm\x01\0\0\0".as_slice()] {
            fs::write(source.join("plugin.component.wasm"), bytes).unwrap();
            let rejected = build_check_report(&source);
            assert_eq!(rejected.status, "rejected");
            assert!(rejected.package.is_none());
            assert!(!rejected.diagnostics.is_empty());
            let human = render_check_report(&rejected, &args).unwrap();
            assert!(human.contains("[rejected]"));
            assert!(human.contains("diagnostics:"));
            assert!(render_pack(&source, Some(&output), &args).is_err());
            assert!(!output.exists());
        }
    }

    #[test]
    fn actual_component_check_pack_and_readback_report_both_digests_and_canonical_size() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("example");
        let output = dir.path().join("example.yoi-plugin");
        let second_output = dir.path().join("second.yoi-plugin");
        let args = PluginCliArgs { json: true };
        built_tool_package(&source);

        let checked: serde_json::Value =
            serde_json::from_str(&render_check(&source, &args).unwrap()).unwrap();
        assert_eq!(checked["status"], "verified");
        assert_eq!(checked["package"]["schema_version"], 2);
        assert_eq!(checked["safety"]["no_network"], true);
        assert_eq!(checked["safety"]["no_plugin_execution"], true);
        assert!(checked["package"].get("digest").is_none());
        assert!(checked["package"].get("permissions").is_none());
        assert_eq!(
            checked["package"]["component"]["imports"],
            serde_json::json!([])
        );
        assert_eq!(
            checked["package"]["component"]["exports"],
            serde_json::json!(["yoi:plugin/tool@2.0.0"])
        );
        assert_eq!(
            checked["package"]["runtime"]["world"],
            "yoi:plugin/tool-plugin@2.0.0"
        );
        assert_ne!(
            checked["package"]["package_digest"],
            checked["package"]["artifact_digest"]
        );
        for field in ["package_digest", "artifact_digest"] {
            assert!(
                checked["package"][field]
                    .as_str()
                    .unwrap()
                    .starts_with("sha256:")
            );
        }

        let packed: serde_json::Value =
            serde_json::from_str(&render_pack(&source, Some(&output), &args).unwrap()).unwrap();
        assert_eq!(packed["status"], "packed");
        let readback: serde_json::Value =
            serde_json::from_str(&render_check(&output, &args).unwrap()).unwrap();
        assert_eq!(readback["status"], "verified");
        for field in [
            "package_digest",
            "artifact_digest",
            "artifact_size",
            "component",
        ] {
            assert_eq!(checked["package"][field], packed["package"][field]);
            assert_eq!(checked["package"][field], readback["package"][field]);
        }
        assert_eq!(
            packed["package"]["artifact_size"].as_u64().unwrap(),
            fs::metadata(&output).unwrap().len()
        );
        render_pack(&source, Some(&second_output), &args).unwrap();
        assert_eq!(
            fs::read(&output).unwrap(),
            fs::read(&second_output).unwrap()
        );

        let human = render_check(&output, &PluginCliArgs::default()).unwrap();
        assert!(human.contains("package_digest=sha256:"));
        assert!(human.contains("artifact_digest=sha256:"));
        assert!(human.contains("artifact_size="));
        assert!(human.contains("static validation of the explicit input path only"));
        let human_pack = render_pack(&source, Some(&output), &PluginCliArgs::default()).unwrap();
        assert!(human_pack.contains("package_digest=sha256:"));
        assert!(human_pack.contains("artifact_digest=sha256:"));
        assert!(human_pack.contains("artifact_size="));
    }

    #[test]
    fn https_component_reports_static_requests_not_host_execution_support() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("https");
        let output = dir.path().join("https.yoi-plugin");
        let args = PluginCliArgs { json: true };
        render_new("rust-component-https-tool", &source, &args).unwrap();
        fs::write(
            source.join("plugin.component.wasm"),
            include_bytes!("../../plugin-pdk/tests/fixtures/https-tool-v2.component.wasm"),
        )
        .unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&render_check(&source, &args).unwrap()).unwrap();
        assert_eq!(report["status"], "verified");
        assert_eq!(
            report["package"]["component"]["world"],
            "yoi:plugin/https-tool-plugin@2.0.0"
        );
        assert_eq!(
            report["package"]["component"]["imports"],
            serde_json::json!([
                "yoi:host/connections@2.0.0",
                "yoi:host/errors@2.0.0",
                "yoi:host/http@2.0.0"
            ])
        );
        assert_eq!(report["package"]["connections"][0]["name"], "api");
        assert_eq!(
            report["package"]["requested_grants"],
            serde_json::json!([
                { "kind": "https", "slot": "api" }
            ])
        );
        assert!(
            report["next_steps"]
                .to_string()
                .contains("does not establish Host implementation, HTTP availability")
        );
        assert_eq!(report["safety"]["no_network"], true);
        assert_eq!(report["safety"]["no_plugin_execution"], true);
        render_pack(&source, Some(&output), &args).unwrap();
        assert_eq!(build_check_report(&output).status, "verified");
    }

    #[test]
    fn unsupported_surface_and_legacy_schema_diagnostics_reach_cli_reports() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("invalid");
        built_tool_package(&source);
        let manifest_path = source.join("plugin.toml");
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        for invalid in [
            manifest.replace("schema_version = 2", "schema_version = 1"),
            manifest.replace("surfaces = [\"tool\"]", "surfaces = [\"service\"]"),
        ] {
            assert_ne!(manifest, invalid);
            fs::write(&manifest_path, invalid).unwrap();
            let rendered = render_check(&source, &PluginCliArgs { json: true }).unwrap();
            let report: serde_json::Value = serde_json::from_str(&rendered).unwrap();
            assert_eq!(report["status"], "rejected");
            assert!(report["package"].is_null());
            assert!(
                !report["diagnostics"][0]["message"]
                    .as_str()
                    .unwrap()
                    .is_empty()
            );
        }
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
