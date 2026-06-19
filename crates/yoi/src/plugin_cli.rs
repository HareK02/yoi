use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use manifest::plugin::{
    PluginConfig, PluginDiagnostic, PluginDiagnosticKind, PluginDiscoveryLimits,
    PluginDiscoveryOptions, PluginDiscoveryReport, PluginPackageManifest, PluginPermission,
    PluginResolution, PluginSourceKind, PluginSurface, ResolvedPlugin, ResolvedPluginRecord,
    SourceQualifiedPluginId, discover_plugins, resolve_enabled_plugins,
};
use manifest::{ProfileResolveOptions, ProfileResolver, ProfileSelector, paths};
use pod::feature::plugin::{PluginStaticInspection, inspect_resolved_plugin_static};
use serde::Serialize;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MAX_LIST_ITEMS: usize = 128;
const MAX_SHOW_DIAGNOSTICS: usize = 48;
const MAX_LIST_DIAGNOSTICS_PER_ITEM: usize = 3;
const MAX_TEXT: usize = 240;

#[derive(Clone, Debug, Default)]
pub(crate) struct PluginCliArgs {
    pub workspace: Option<PathBuf>,
    pub profile: Option<String>,
    pub json: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum PluginCliCommand {
    List(PluginCliArgs),
    Show {
        reference: String,
        args: PluginCliArgs,
    },
}

pub(crate) fn run(command: PluginCliCommand) -> Result<()> {
    let rendered = match command {
        PluginCliCommand::List(args) => render_list(&args)?,
        PluginCliCommand::Show { reference, args } => render_show(&reference, &args)?,
    };
    print!("{rendered}");
    Ok(())
}

fn render_list(args: &PluginCliArgs) -> Result<String> {
    let snapshot = build_snapshot(args)?;
    if args.json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(&snapshot)?));
    }

    render_list_snapshot_human(&snapshot)
}

fn render_list_snapshot_human(snapshot: &PluginInspectionSnapshot) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "plugin packages (workspace: {})",
        snapshot.workspace.display()
    )?;
    if snapshot.items.is_empty() {
        writeln!(out, "  (none)")?;
        return Ok(out);
    }

    for item in snapshot.items.iter().take(MAX_LIST_ITEMS) {
        writeln!(
            out,
            "- {} [{}] version={} schema_version={} api_version={} package_path={} digest={} source={} enabled_surfaces={} tools={} diagnostics={}",
            item.reference,
            item.status,
            item.version.as_deref().unwrap_or("<unknown>"),
            item.schema_version
                .map(|version| version.to_string())
                .unwrap_or_else(|| "<unknown>".to_string()),
            item.api_version
                .map(|version| version.to_string())
                .unwrap_or_else(|| "<unknown>".to_string()),
            item.package_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string()),
            item.digest.as_deref().unwrap_or("<unknown>"),
            item.source.as_deref().unwrap_or("<unknown>"),
            join_or_none(&item.enabled_surfaces),
            item.tools.len(),
            item.diagnostics.len()
        )?;
        for diagnostic in item.diagnostics.iter().take(MAX_LIST_DIAGNOSTICS_PER_ITEM) {
            writeln!(out, "    diagnostic: {}", diagnostic.message)?;
        }
    }
    if snapshot.items.len() > MAX_LIST_ITEMS {
        writeln!(
            out,
            "  ... {} more plugin package(s) omitted",
            snapshot.items.len() - MAX_LIST_ITEMS
        )?;
    }
    Ok(out)
}

fn render_show(reference: &str, args: &PluginCliArgs) -> Result<String> {
    let snapshot = build_snapshot(args)?;
    let item = select_item(&snapshot, reference)?;
    if args.json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(item)?));
    }

    render_item_human(item)
}

fn render_item_human(item: &PluginInspectionItem) -> Result<String> {
    let mut out = String::new();
    writeln!(out, "plugin {}", item.reference)?;
    writeln!(out, "  status: {}", item.status)?;
    writeln!(
        out,
        "  source: {}",
        item.source.as_deref().unwrap_or("<unknown>")
    )?;
    writeln!(
        out,
        "  package: {}",
        item.package.as_deref().unwrap_or("<unknown>")
    )?;
    writeln!(
        out,
        "  package_path: {}",
        item.package_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string())
    )?;
    writeln!(
        out,
        "  schema_version: {}",
        item.schema_version
            .map(|version| version.to_string())
            .unwrap_or_else(|| "<unknown>".to_string())
    )?;
    writeln!(
        out,
        "  api_version: {}",
        item.api_version
            .map(|version| version.to_string())
            .unwrap_or_else(|| "<unknown>".to_string())
    )?;
    writeln!(
        out,
        "  version: {}",
        item.version.as_deref().unwrap_or("<unknown>")
    )?;
    writeln!(
        out,
        "  digest: {}",
        item.digest.as_deref().unwrap_or("<unknown>")
    )?;
    writeln!(out, "  configured: {}", item.configured)?;
    writeln!(out, "  discovered: {}", item.discovered)?;
    writeln!(out, "  resolved: {}", item.resolved)?;
    writeln!(out, "  static_eligible: {}", item.static_eligible)?;

    writeln!(
        out,
        "  declared_surfaces: {}",
        join_or_none(&item.declared_surfaces)
    )?;
    writeln!(
        out,
        "  enabled_surfaces: {}",
        join_or_none(&item.enabled_surfaces)
    )?;
    writeln!(
        out,
        "  requested_permissions: {}",
        join_or_none(&item.requested_permissions)
    )?;
    writeln!(
        out,
        "  configured_grants: {}",
        join_or_none(&item.configured_grants)
    )?;

    if let Some(runtime) = &item.static_runtime {
        writeln!(
            out,
            "  runtime: {} eligible={}",
            runtime.runtime.status, runtime.runtime.eligible
        )?;
        if let Some(diagnostic) = &runtime.runtime.diagnostic {
            writeln!(out, "    diagnostic: {diagnostic}")?;
        }
        if !runtime.host_apis.is_empty() {
            writeln!(out, "  host_api_permissions:")?;
            for api in &runtime.host_apis {
                writeln!(
                    out,
                    "    - {} requested={} granted={} eligible={}",
                    api.permission, api.requested, api.granted, api.eligible
                )?;
                if let Some(diagnostic) = &api.diagnostic {
                    writeln!(out, "      diagnostic: {diagnostic}")?;
                }
            }
        }
    }

    if item.tools.is_empty() {
        writeln!(out, "  tools: (none)")?;
    } else {
        writeln!(out, "  tools:")?;
        for tool in &item.tools {
            writeln!(
                out,
                "    - {} permission={} requested={} granted={} eligible={} external_write={}",
                tool.name,
                tool.permission,
                tool.requested,
                tool.granted,
                tool.eligible,
                tool.external_write
            )?;
            if let Some(diagnostic) = &tool.diagnostic {
                writeln!(out, "      diagnostic: {diagnostic}")?;
            }
        }
    }

    if item.diagnostics.is_empty() {
        writeln!(out, "  diagnostics: (none)")?;
    } else {
        writeln!(out, "  diagnostics:")?;
        for diagnostic in item.diagnostics.iter().take(MAX_SHOW_DIAGNOSTICS) {
            writeln!(out, "    - [{}] {}", diagnostic.kind, diagnostic.message)?;
        }
        if item.diagnostics.len() > MAX_SHOW_DIAGNOSTICS {
            writeln!(
                out,
                "    ... {} more diagnostic(s) omitted",
                item.diagnostics.len() - MAX_SHOW_DIAGNOSTICS
            )?;
        }
    }

    Ok(out)
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        values.join(", ")
    }
}

fn build_snapshot(args: &PluginCliArgs) -> Result<PluginInspectionSnapshot> {
    let workspace = args
        .workspace
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)?;
    let workspace = fs::canonicalize(&workspace).map_err(|error| {
        format!(
            "workspace `{}` does not exist: {error}",
            workspace.display()
        )
    })?;
    let config = load_plugin_config(args, &workspace)?;

    let options = PluginDiscoveryOptions {
        workspace_root: workspace.clone(),
        user_data_home: paths::data_dir(),
        limits: PluginDiscoveryLimits::default(),
    };
    let discovery = discover_plugins(&options);
    let resolution = resolve_enabled_plugins(&config, &discovery);

    Ok(snapshot_from_resolution(
        workspace,
        &config,
        &discovery,
        &resolution,
    ))
}

fn load_plugin_config(args: &PluginCliArgs, workspace: &Path) -> Result<PluginConfig> {
    let selector = args
        .profile
        .as_deref()
        .map(ProfileSelector::parse_cli)
        .unwrap_or(ProfileSelector::Default);
    let resolved = ProfileResolver::new()
        .with_workspace_base(workspace)
        .resolve(
            &selector,
            ProfileResolveOptions::with_pod_name("plugin-inspect"),
        )?;
    Ok(resolved.manifest.plugins)
}

fn snapshot_from_resolution(
    workspace: PathBuf,
    config: &PluginConfig,
    discovery: &PluginDiscoveryReport,
    resolution: &PluginResolution,
) -> PluginInspectionSnapshot {
    let mut builders: BTreeMap<String, ItemBuilder> = BTreeMap::new();

    for package in &discovery.packages {
        let key = package.identity.to_string();
        let builder = builders
            .entry(key.clone())
            .or_insert_with(|| ItemBuilder::new(key));
        builder.discovered = true;
        builder.source = Some(package.identity.source.to_string());
        builder.package = Some(package.package_label.clone());
        builder.package_path = Some(package.package_path.clone());
        builder.digest = Some(package.digest.clone());
        builder.version = Some(package.manifest.version.clone());
        builder.schema_version = Some(package.manifest.schema_version);
        builder.api_version = Some(package.manifest.schema_version);
        builder.declared_surfaces = surface_strings(package.manifest.surfaces.iter().copied());
        builder.requested_permissions = permission_strings(&package.manifest.permissions);
        builder.tools = package
            .manifest
            .tools
            .iter()
            .map(|tool| ToolSummary {
                name: tool.name.clone(),
                permission: PluginPermission::tool(&tool.name).label(),
                requested: permission_requested(
                    &package.manifest,
                    &PluginPermission::tool(&tool.name),
                ),
                granted: false,
                eligible: false,
                external_write: tool.external_write,
                diagnostic: Some("plugin is discovered but not enabled".to_string()),
            })
            .collect();
    }

    for enablement in &config.enabled {
        let key = enablement.id.clone();
        let builder = builders
            .entry(key.clone())
            .or_insert_with(|| ItemBuilder::new(key));
        builder.configured = true;
        builder.enabled_surfaces = surface_strings(enablement.surfaces.iter().copied());
        builder.configured_grants = permission_strings(&enablement.grants.permissions);
        if let Ok(identity) = SourceQualifiedPluginId::parse(&enablement.id) {
            builder
                .source
                .get_or_insert_with(|| identity.source.to_string());
            builder.package_path.get_or_insert_with(|| {
                package_path_for_source(
                    &workspace,
                    identity.source,
                    &format!("{}.yoi-plugin", identity.local_id),
                )
            });
        }
    }

    for resolved in &resolution.resolved {
        let key = resolved.identity.to_string();
        let builder = builders
            .entry(key.clone())
            .or_insert_with(|| ItemBuilder::new(key));
        fill_resolved(builder, resolved);
    }

    for diagnostic in discovery
        .diagnostics
        .iter()
        .chain(resolution.diagnostics.iter())
    {
        let rendered = DiagnosticSummary::from(diagnostic);
        if let Some(reference) = diagnostic_reference(diagnostic) {
            builders
                .entry(reference.clone())
                .or_insert_with(|| ItemBuilder::new(reference))
                .diagnostics
                .push(rendered);
        } else if let (Some(source), Some(package)) =
            (diagnostic.source, diagnostic.package.as_ref())
        {
            let local_id = package_local_id(package);
            let key = format!("{source}:{local_id}");
            let builder = builders
                .entry(key.clone())
                .or_insert_with(|| ItemBuilder::new(key));
            builder.source.get_or_insert_with(|| source.to_string());
            builder.package.get_or_insert_with(|| package.clone());
            builder
                .package_path
                .get_or_insert_with(|| package_path_for_source(&workspace, source, package));
            builder.diagnostics.push(rendered);
        } else {
            let key = "<global>".to_string();
            builders
                .entry(key.clone())
                .or_insert_with(|| ItemBuilder::new(key))
                .diagnostics
                .push(rendered);
        }
    }

    let items = builders
        .into_values()
        .map(ItemBuilder::finish)
        .take(MAX_LIST_ITEMS + 1)
        .collect();

    PluginInspectionSnapshot { workspace, items }
}

fn fill_resolved(builder: &mut ItemBuilder, resolved: &ResolvedPlugin) {
    builder.configured = true;
    builder.discovered = true;
    builder.resolved = true;
    builder.source = Some(resolved.identity.source.to_string());
    builder.package = Some(resolved.package_label.clone());
    builder.package_path = Some(resolved.package_path.clone());
    builder.digest = Some(resolved.digest.clone());
    builder.version = Some(resolved.manifest.version.clone());
    builder.schema_version = Some(resolved.manifest.schema_version);
    builder.api_version = Some(resolved.manifest.schema_version);
    builder.declared_surfaces = surface_strings(resolved.manifest.surfaces.iter().copied());
    builder.enabled_surfaces = surface_strings(resolved.enabled_surfaces.iter().copied());
    builder.requested_permissions = permission_strings(&resolved.manifest.permissions);
    builder.configured_grants = permission_strings(&resolved.grants.permissions);

    let record = ResolvedPluginRecord::from_resolved(resolved);
    let static_runtime = inspect_resolved_plugin_static(&record);
    for diagnostic in static_runtime
        .runtime
        .diagnostic
        .iter()
        .chain(
            static_runtime
                .host_apis
                .iter()
                .filter_map(|api| api.diagnostic.as_ref()),
        )
        .chain(
            static_runtime
                .tools
                .iter()
                .filter_map(|tool| tool.diagnostic.as_ref()),
        )
    {
        builder.diagnostics.push(DiagnosticSummary {
            kind: "static_eligibility".to_string(),
            message: bound_text(diagnostic.clone()),
        });
    }
    builder.static_eligible = static_runtime.statically_eligible();
    builder.tools = static_runtime
        .tools
        .iter()
        .map(|tool| ToolSummary {
            name: tool.name.clone(),
            permission: tool.permission.clone(),
            requested: tool.requested,
            granted: tool.granted,
            eligible: tool.eligible,
            external_write: tool.external_write,
            diagnostic: tool.diagnostic.clone().map(bound_text),
        })
        .collect();
    builder.static_runtime = Some(static_runtime);
}

fn diagnostic_reference(diagnostic: &PluginDiagnostic) -> Option<String> {
    diagnostic.identity.clone()
}

fn select_item<'a>(
    snapshot: &'a PluginInspectionSnapshot,
    reference: &str,
) -> Result<&'a PluginInspectionItem> {
    if let Some(item) = snapshot
        .items
        .iter()
        .find(|item| item.reference == reference)
    {
        return Ok(item);
    }

    let local_matches: Vec<_> = snapshot
        .items
        .iter()
        .filter(|item| item.local_ref.as_deref() == Some(reference))
        .collect();
    match local_matches.as_slice() {
        [item] => Ok(item),
        [] => Err(format!(
            "plugin package `{}` was not found",
            bound_text(reference.to_string())
        )
        .into()),
        _ => Err(format!(
            "plugin package reference `{}` is ambiguous; use a source-qualified ref",
            bound_text(reference.to_string())
        )
        .into()),
    }
}

fn surface_strings(surfaces: impl Iterator<Item = PluginSurface>) -> Vec<String> {
    let mut values: Vec<_> = surfaces.map(|surface| surface.to_string()).collect();
    values.sort();
    values.dedup();
    values
}

fn permission_strings(permissions: &[PluginPermission]) -> Vec<String> {
    let mut values: Vec<_> = permissions
        .iter()
        .map(|permission| permission.label())
        .collect();
    values.sort();
    values.dedup();
    values
}

fn permission_requested(manifest: &PluginPackageManifest, permission: &PluginPermission) -> bool {
    manifest
        .permissions
        .iter()
        .any(|requested| requested == permission)
}

fn package_local_id(package_label: &str) -> String {
    package_label
        .strip_suffix(".yoi-plugin")
        .unwrap_or(package_label)
        .to_string()
}

fn package_path_for_source(
    workspace: &Path,
    source: PluginSourceKind,
    package_label: &str,
) -> PathBuf {
    match source {
        PluginSourceKind::Project => workspace.join(".yoi/plugins").join(package_label),
        PluginSourceKind::User => paths::data_dir()
            .unwrap_or_else(|| PathBuf::from("<unavailable-user-data-dir>"))
            .join("yoi/plugins")
            .join(package_label),
        PluginSourceKind::Builtin => PathBuf::from("<builtin>").join(package_label),
    }
}

fn local_ref(reference: &str) -> Option<String> {
    SourceQualifiedPluginId::parse(reference)
        .ok()
        .map(|identity| identity.local_id.to_string())
}

fn bound_text(mut text: String) -> String {
    text = text.replace('\n', " ");
    if text.chars().count() > MAX_TEXT {
        text = text.chars().take(MAX_TEXT).collect::<String>();
        text.push('…');
    }
    text
}

#[derive(Clone, Debug, Serialize)]
struct PluginInspectionSnapshot {
    workspace: PathBuf,
    items: Vec<PluginInspectionItem>,
}

#[derive(Clone, Debug, Serialize)]
struct PluginInspectionItem {
    reference: String,
    local_ref: Option<String>,
    status: String,
    source: Option<String>,
    package: Option<String>,
    package_path: Option<PathBuf>,
    version: Option<String>,
    schema_version: Option<u32>,
    api_version: Option<u32>,
    digest: Option<String>,
    configured: bool,
    discovered: bool,
    resolved: bool,
    static_eligible: bool,
    declared_surfaces: Vec<String>,
    enabled_surfaces: Vec<String>,
    requested_permissions: Vec<String>,
    configured_grants: Vec<String>,
    tools: Vec<ToolSummary>,
    static_runtime: Option<PluginStaticInspection>,
    diagnostics: Vec<DiagnosticSummary>,
}

#[derive(Clone, Debug, Serialize)]
struct ToolSummary {
    name: String,
    permission: String,
    requested: bool,
    granted: bool,
    eligible: bool,
    external_write: bool,
    diagnostic: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct DiagnosticSummary {
    kind: String,
    message: String,
}

impl DiagnosticSummary {
    fn from(diagnostic: &PluginDiagnostic) -> Self {
        Self {
            kind: diagnostic_kind(&diagnostic.kind).to_string(),
            message: bound_text(diagnostic.message.clone()),
        }
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

#[derive(Clone, Debug)]
struct ItemBuilder {
    reference: String,
    configured: bool,
    discovered: bool,
    resolved: bool,
    source: Option<String>,
    package: Option<String>,
    package_path: Option<PathBuf>,
    version: Option<String>,
    schema_version: Option<u32>,
    api_version: Option<u32>,
    digest: Option<String>,
    static_eligible: bool,
    declared_surfaces: Vec<String>,
    enabled_surfaces: Vec<String>,
    requested_permissions: Vec<String>,
    configured_grants: Vec<String>,
    tools: Vec<ToolSummary>,
    static_runtime: Option<PluginStaticInspection>,
    diagnostics: Vec<DiagnosticSummary>,
}

impl ItemBuilder {
    fn new(reference: String) -> Self {
        Self {
            reference,
            configured: false,
            discovered: false,
            resolved: false,
            source: None,
            package: None,
            package_path: None,
            version: None,
            schema_version: None,
            api_version: None,
            digest: None,
            static_eligible: false,
            declared_surfaces: Vec::new(),
            enabled_surfaces: Vec::new(),
            requested_permissions: Vec::new(),
            configured_grants: Vec::new(),
            tools: Vec::new(),
            static_runtime: None,
            diagnostics: Vec::new(),
        }
    }

    fn finish(mut self) -> PluginInspectionItem {
        self.diagnostics.sort_by(|left, right| {
            (left.kind.as_str(), left.message.as_str())
                .cmp(&(right.kind.as_str(), right.message.as_str()))
        });
        let usable_tool = self.tools.iter().any(|tool| tool.eligible);
        let rejected_tool = self
            .tools
            .iter()
            .any(|tool| !tool.eligible || tool.diagnostic.is_some());
        let static_runtime_rejected = self.static_runtime.as_ref().is_some_and(|runtime| {
            !runtime.runtime.eligible
                || runtime.runtime.diagnostic.is_some()
                || runtime
                    .host_apis
                    .iter()
                    .any(|api| !api.eligible || api.diagnostic.is_some())
        });
        let has_diagnostic =
            !self.diagnostics.is_empty() || rejected_tool || static_runtime_rejected;
        let has_non_missing_diagnostic = self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind != "missing");
        let status = if self.resolved {
            if usable_tool && has_diagnostic {
                "partial"
            } else if usable_tool || (self.static_eligible && !self.enabled_surfaces.is_empty()) {
                "active"
            } else {
                "rejected"
            }
        } else if self.discovered && !self.configured {
            "disabled"
        } else if self.configured && !self.discovered {
            if has_non_missing_diagnostic {
                "rejected"
            } else {
                "missing"
            }
        } else {
            "rejected"
        }
        .to_string();
        let local_ref = local_ref(&self.reference);
        PluginInspectionItem {
            reference: self.reference,
            local_ref,
            status,
            source: self.source,
            package: self.package,
            package_path: self.package_path,
            version: self.version,
            schema_version: self.schema_version,
            api_version: self.api_version,
            digest: self.digest,
            configured: self.configured,
            discovered: self.discovered,
            resolved: self.resolved,
            static_eligible: self.static_eligible,
            declared_surfaces: self.declared_surfaces,
            enabled_surfaces: self.enabled_surfaces,
            requested_permissions: self.requested_permissions,
            configured_grants: self.configured_grants,
            tools: self.tools,
            static_runtime: self.static_runtime,
            diagnostics: self.diagnostics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::plugin::{PluginEnablementConfig, PluginExactVersion, PluginGrantConfig};
    use tempfile::tempdir;

    #[test]
    fn active_list_and_show_json_are_structured_and_non_executing() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        write_plugin_package(workspace, "echo");
        let config = enabled_config(workspace, true, None);
        let snapshot = inspect_snapshot(workspace, &config);

        assert_eq!(snapshot.items.len(), 1);
        let item = select_item(&snapshot, "echo").unwrap();
        assert_eq!(item.status, "active");
        assert_eq!(item.tools[0].name, "Echo");
        assert!(item.static_eligible);
        assert_eq!(item.package.as_deref(), Some("echo.yoi-plugin"));
        assert_eq!(item.schema_version, Some(1));
        assert_eq!(item.api_version, Some(1));
        assert_eq!(
            item.package_path.as_deref(),
            Some(workspace.join(".yoi/plugins/echo.yoi-plugin").as_path())
        );

        let list_json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(list_json["items"][0]["status"], "active");
        assert_eq!(list_json["items"][0]["schema_version"], 1);
        assert_eq!(list_json["items"][0]["api_version"], 1);
        assert_eq!(
            list_json["items"][0]["package_path"],
            workspace
                .join(".yoi/plugins/echo.yoi-plugin")
                .display()
                .to_string()
        );
        assert_eq!(list_json["items"][0]["enabled_surfaces"][0], "tool");
        assert_eq!(list_json["items"][0]["tools"][0]["granted"], true);

        let show_json = serde_json::to_value(item).unwrap();
        assert_eq!(show_json["status"], "active");
        assert_eq!(show_json["schema_version"], 1);
        assert_eq!(show_json["api_version"], 1);
        assert_eq!(
            show_json["package_path"],
            workspace
                .join(".yoi/plugins/echo.yoi-plugin")
                .display()
                .to_string()
        );
        assert_eq!(show_json["configured_grants"][0], "surfaces.tool");
        assert_eq!(show_json["tools"][0]["permission"], "tool.Echo");

        let show = render_item_human(item).unwrap();
        assert!(show.contains("status: active"));
        assert!(show.contains("schema_version: 1"));
        assert!(show.contains("api_version: 1"));
        assert!(show.contains("package_path:"));
        assert!(show.contains("echo.yoi-plugin"));
        assert!(show.contains("configured_grants: surfaces.tool, tool.Echo"));
    }

    #[test]
    fn human_list_uses_required_status_vocabulary() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        write_plugin_package(workspace, "echo");
        write_plugin_package(workspace, "spare");
        let bad_digest = write_plugin_package(workspace, "bad");
        let mut config = enabled_config(workspace, true, None);
        config.enabled.push(PluginEnablementConfig {
            id: "project:missing".to_string(),
            digest: None,
            version: Some(PluginExactVersion("0.1.0".to_string())),
            surfaces: vec![PluginSurface::Tool],
            grants: PluginGrantConfig {
                id: Some("project:missing".to_string()),
                version: Some(PluginExactVersion("0.1.0".to_string())),
                digest: None,
                permissions: vec![
                    PluginPermission::surface(PluginSurface::Tool),
                    PluginPermission::tool("Echo"),
                ],
            },
            config: None,
        });
        config.enabled.push(PluginEnablementConfig {
            id: "project:bad".to_string(),
            digest: Some("sha256:0000".to_string()),
            version: Some(PluginExactVersion("0.1.0".to_string())),
            surfaces: vec![PluginSurface::Tool],
            grants: PluginGrantConfig {
                id: Some("project:bad".to_string()),
                version: Some(PluginExactVersion("0.1.0".to_string())),
                digest: Some(bad_digest),
                permissions: vec![
                    PluginPermission::surface(PluginSurface::Tool),
                    PluginPermission::tool("Echo"),
                ],
            },
            config: None,
        });

        let snapshot = inspect_snapshot(workspace, &config);
        let statuses: std::collections::BTreeSet<_> = snapshot
            .items
            .iter()
            .map(|item| item.status.as_str())
            .collect();
        assert_eq!(
            statuses,
            std::collections::BTreeSet::from(["active", "disabled", "missing", "rejected"])
        );
        let output = render_list_snapshot_human(&snapshot).unwrap();

        assert!(output.contains("project:echo [active]"));
        assert!(output.contains("project:spare [disabled]"));
        assert!(output.contains("project:bad [rejected]"));
        assert!(output.contains("project:missing [missing]"));
        assert!(output.contains("schema_version=1"));
        assert!(output.contains("api_version=1"));
        assert!(output.contains("package_path="));
        assert!(output.contains("echo.yoi-plugin"));
        assert!(output.contains("missing.yoi-plugin"));
        assert!(output.contains("enabled_surfaces=tool"));
        assert!(!output.contains("enabled-with-diagnostics"));
        assert!(!output.contains("configured-"));
    }

    #[test]
    fn missing_ref_is_bounded_error() {
        let snapshot = PluginInspectionSnapshot {
            workspace: PathBuf::from("/tmp/workspace"),
            items: Vec::new(),
        };
        let error = select_item(&snapshot, "missing").unwrap_err().to_string();
        assert!(error.contains("missing"));
        assert!(error.len() < 120);
    }

    #[test]
    fn grant_mismatch_is_inspectable() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        let digest = write_plugin_package(workspace, "echo");
        let mut config = enabled_config(workspace, false, Some(digest));
        config.enabled[0].grants.permissions = vec![PluginPermission::tool("Other")];

        let snapshot = inspect_snapshot(workspace, &config);
        let item = select_item(&snapshot, "project:echo").unwrap();
        assert_eq!(item.status, "rejected");
        assert!(!item.static_eligible);
        assert!(
            item.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == "static_eligibility")
        );
    }

    #[test]
    fn partial_status_represents_mixed_tool_usability() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        let digest = write_dual_tool_package(workspace, "echo");
        let mut config = enabled_config(workspace, true, Some(digest));
        config.enabled[0].grants.permissions = vec![
            PluginPermission::surface(PluginSurface::Tool),
            PluginPermission::tool("Echo"),
        ];

        let snapshot = inspect_snapshot(workspace, &config);
        let item = select_item(&snapshot, "project:echo").unwrap();

        assert_eq!(item.status, "partial");
        assert!(
            item.tools
                .iter()
                .any(|tool| tool.name == "Echo" && tool.eligible)
        );
        assert!(
            item.tools
                .iter()
                .any(|tool| tool.name == "Other" && !tool.eligible)
        );
    }

    #[test]
    fn invalid_manifest_and_digest_mismatch_are_rejected_diagnostics() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        fs::create_dir_all(workspace.join(".yoi/plugins")).unwrap();
        write_stored_zip(
            &workspace.join(".yoi/plugins/invalid.yoi-plugin"),
            &[("plugin.toml", b"not = [valid")],
        );
        let invalid_snapshot = inspect_snapshot(workspace, &PluginConfig::default());
        let invalid_output = render_list_snapshot_human(&invalid_snapshot).unwrap();
        assert!(invalid_output.contains("[rejected]"));
        assert!(invalid_output.contains("diagnostic:"));

        let digest = write_plugin_package(workspace, "bad");
        let mut config = PluginConfig::default();
        config.enabled.push(PluginEnablementConfig {
            id: "project:bad".to_string(),
            digest: Some("sha256:0000".to_string()),
            version: Some(PluginExactVersion("0.1.0".to_string())),
            surfaces: vec![PluginSurface::Tool],
            grants: PluginGrantConfig {
                id: Some("project:bad".to_string()),
                version: Some(PluginExactVersion("0.1.0".to_string())),
                digest: Some(digest),
                permissions: vec![
                    PluginPermission::surface(PluginSurface::Tool),
                    PluginPermission::tool("Echo"),
                ],
            },
            config: None,
        });
        let snapshot = inspect_snapshot(workspace, &config);
        let item = select_item(&snapshot, "project:bad").unwrap();
        assert_eq!(item.status, "rejected");
        assert!(
            item.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == "digest")
        );
    }

    #[test]
    fn configured_invalid_or_incompatible_package_is_rejected_not_missing() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        fs::create_dir_all(workspace.join(".yoi/plugins")).unwrap();
        write_stored_zip(
            &workspace.join(".yoi/plugins/invalid.yoi-plugin"),
            &[("plugin.toml", b"not = [valid")],
        );
        let incompatible_manifest = plugin_manifest_with_schema("incompat", "Echo", 999);
        write_stored_zip(
            &workspace.join(".yoi/plugins/incompat.yoi-plugin"),
            &[
                ("plugin.toml", incompatible_manifest.as_bytes()),
                ("plugin.wasm", b"not wasm"),
            ],
        );
        let mut config = PluginConfig::default();
        config.enabled.push(enablement_without_digest(
            "project:invalid",
            "0.1.0",
            &["Echo"],
        ));
        config.enabled.push(enablement_without_digest(
            "project:incompat",
            "0.1.0",
            &["Echo"],
        ));

        let snapshot = inspect_snapshot(workspace, &config);
        let invalid = select_item(&snapshot, "project:invalid").unwrap();
        let incompatible = select_item(&snapshot, "project:incompat").unwrap();

        assert_eq!(invalid.status, "rejected");
        assert_eq!(incompatible.status, "rejected");
        assert!(invalid.configured);
        assert!(!invalid.discovered);
        assert!(incompatible.configured);
        assert!(!incompatible.discovered);
        assert!(!invalid.diagnostics.is_empty());
        assert!(
            incompatible
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == "api")
        );

        let list_json = serde_json::to_value(&snapshot).unwrap();
        assert!(list_json["items"].as_array().unwrap().iter().any(|item| {
            item["reference"] == "project:invalid"
                && item["status"] == "rejected"
                && item["package_path"]
                    .as_str()
                    .unwrap_or_default()
                    .ends_with(".yoi/plugins/invalid.yoi-plugin")
        }));
        let show_json = serde_json::to_value(incompatible).unwrap();
        assert_eq!(show_json["status"], "rejected");
        assert!(
            show_json["diagnostics"][0]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("unsupported")
        );

        let list_output = render_list_snapshot_human(&snapshot).unwrap();
        assert!(list_output.contains("project:invalid [rejected]"));
        assert!(list_output.contains("project:incompat [rejected]"));
        assert!(!list_output.contains("project:invalid [missing]"));
        let show_output = render_item_human(invalid).unwrap();
        assert!(show_output.contains("status: rejected"));
        assert!(show_output.contains("diagnostics:"));
    }

    #[test]
    fn invalid_tool_schema_and_name_are_rejected_in_json_and_human_output() {
        let dir = tempdir().unwrap();
        let workspace = dir.path();
        let bad_schema_manifest = plugin_manifest("bad_schema", "Echo", "string", &["Echo"]);
        let bad_name_manifest = plugin_manifest("bad_name", "Bad Tool", "object", &["Bad Tool"]);
        let bad_schema_digest =
            write_plugin_manifest(workspace, "bad_schema", &bad_schema_manifest);
        let bad_name_digest = write_plugin_manifest(workspace, "bad_name", &bad_name_manifest);
        let mut config = PluginConfig::default();
        config.enabled.push(enablement(
            "project:bad_schema",
            "0.1.0",
            bad_schema_digest,
            &["Echo"],
        ));
        config.enabled.push(enablement(
            "project:bad_name",
            "0.1.0",
            bad_name_digest,
            &["Bad Tool"],
        ));

        let snapshot = inspect_snapshot(workspace, &config);
        let bad_schema = select_item(&snapshot, "project:bad_schema").unwrap();
        let bad_name = select_item(&snapshot, "project:bad_name").unwrap();

        assert_eq!(bad_schema.status, "rejected");
        assert_eq!(bad_name.status, "rejected");
        assert!(!bad_schema.tools[0].eligible);
        assert!(!bad_name.tools[0].eligible);

        let list_json = serde_json::to_value(&snapshot).unwrap();
        assert!(list_json["items"].as_array().unwrap().iter().any(|item| {
            item["reference"] == "project:bad_schema"
                && item["status"] == "rejected"
                && item["tools"][0]["diagnostic"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("invalid input_schema")
        }));
        let show_json = serde_json::to_value(bad_name).unwrap();
        assert_eq!(show_json["status"], "rejected");
        assert!(
            show_json["tools"][0]["diagnostic"]
                .as_str()
                .unwrap_or_default()
                .contains("invalid name")
        );

        let list_output = render_list_snapshot_human(&snapshot).unwrap();
        assert!(list_output.contains("project:bad_schema [rejected]"));
        assert!(list_output.contains("project:bad_name [rejected]"));
        let show_output = render_item_human(bad_schema).unwrap();
        assert!(show_output.contains("invalid input_schema"));
        assert!(show_output.contains("eligible=false"));
    }

    #[test]
    fn ambiguous_ref_is_bounded_error() {
        let snapshot = PluginInspectionSnapshot {
            workspace: PathBuf::from("/tmp/workspace"),
            items: vec![
                ItemBuilder::new("project:echo".to_string()).finish(),
                ItemBuilder::new("user:echo".to_string()).finish(),
            ],
        };

        let error = select_item(&snapshot, "echo").unwrap_err().to_string();

        assert!(error.contains("ambiguous"));
        assert!(error.len() < 160);
    }

    fn inspect_snapshot(workspace: &Path, config: &PluginConfig) -> PluginInspectionSnapshot {
        let discovery = discover_plugins(&PluginDiscoveryOptions {
            workspace_root: workspace.to_path_buf(),
            user_data_home: None,
            limits: PluginDiscoveryLimits::default(),
        });
        let resolution = resolve_enabled_plugins(config, &discovery);
        snapshot_from_resolution(workspace.to_path_buf(), config, &discovery, &resolution)
    }

    fn enabled_config(
        workspace: &Path,
        include_digest: bool,
        digest_override: Option<String>,
    ) -> PluginConfig {
        let digest = digest_override.unwrap_or_else(|| write_plugin_package(workspace, "echo"));
        PluginConfig {
            enabled: vec![PluginEnablementConfig {
                id: "project:echo".to_string(),
                digest: include_digest.then_some(digest.clone()),
                version: Some(PluginExactVersion("0.1.0".to_string())),
                surfaces: vec![PluginSurface::Tool],
                grants: PluginGrantConfig {
                    id: Some("project:echo".to_string()),
                    version: Some(PluginExactVersion("0.1.0".to_string())),
                    digest: Some(digest),
                    permissions: vec![
                        PluginPermission::surface(PluginSurface::Tool),
                        PluginPermission::tool("Echo"),
                    ],
                },
                config: None,
            }],
            ..PluginConfig::default()
        }
    }

    fn enablement(
        id: &str,
        version: &str,
        digest: String,
        tool_permissions: &[&str],
    ) -> PluginEnablementConfig {
        let mut permissions = vec![PluginPermission::surface(PluginSurface::Tool)];
        permissions.extend(
            tool_permissions
                .iter()
                .map(|tool_name| PluginPermission::tool(*tool_name)),
        );
        PluginEnablementConfig {
            id: id.to_string(),
            digest: Some(digest.clone()),
            version: Some(PluginExactVersion(version.to_string())),
            surfaces: vec![PluginSurface::Tool],
            grants: PluginGrantConfig {
                id: Some(id.to_string()),
                version: Some(PluginExactVersion(version.to_string())),
                digest: Some(digest),
                permissions,
            },
            config: None,
        }
    }

    fn enablement_without_digest(
        id: &str,
        version: &str,
        tool_permissions: &[&str],
    ) -> PluginEnablementConfig {
        let mut permissions = vec![PluginPermission::surface(PluginSurface::Tool)];
        permissions.extend(
            tool_permissions
                .iter()
                .map(|tool_name| PluginPermission::tool(*tool_name)),
        );
        PluginEnablementConfig {
            id: id.to_string(),
            digest: None,
            version: Some(PluginExactVersion(version.to_string())),
            surfaces: vec![PluginSurface::Tool],
            grants: PluginGrantConfig {
                id: Some(id.to_string()),
                version: Some(PluginExactVersion(version.to_string())),
                digest: None,
                permissions,
            },
            config: None,
        }
    }

    fn plugin_manifest(
        id: &str,
        tool_name: &str,
        schema_type: &str,
        permission_tools: &[&str],
    ) -> String {
        plugin_manifest_with_schema_and_tool(id, tool_name, schema_type, permission_tools, 1)
    }

    fn plugin_manifest_with_schema(id: &str, tool_name: &str, schema_version: u32) -> String {
        plugin_manifest_with_schema_and_tool(id, tool_name, "object", &[tool_name], schema_version)
    }

    fn plugin_manifest_with_schema_and_tool(
        id: &str,
        tool_name: &str,
        schema_type: &str,
        permission_tools: &[&str],
        schema_version: u32,
    ) -> String {
        let permissions = permission_tools
            .iter()
            .map(|tool| format!(r#"{{ kind = "tool", name = "{tool}" }}"#))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"
schema_version = {schema_version}
id = "{id}"
name = "{id}"
version = "0.1.0"
surfaces = ["tool"]
permissions = [{{ kind = "surface", surface = "tool" }}, {permissions}]

[runtime]
kind = "wasm"
entry = "plugin.wasm"
abi = "yoi-plugin-wasm-1"

[[tools]]
name = "{tool_name}"
description = "Test tool"
input_schema = {{ type = "{schema_type}" }}
"#
        )
    }

    fn write_plugin_package(workspace: &Path, id: &str) -> String {
        let manifest = format!(
            r#"
schema_version = 1
id = "{id}"
name = "{id}"
version = "0.1.0"
surfaces = ["tool"]
permissions = [{{ kind = "surface", surface = "tool" }}, {{ kind = "tool", name = "Echo" }}]

[runtime]
kind = "wasm"
entry = "plugin.wasm"
abi = "yoi-plugin-wasm-1"

[[tools]]
name = "Echo"
description = "Echo input"
input_schema = {{ type = "object" }}
"#
        );
        write_plugin_manifest(workspace, id, &manifest)
    }

    fn write_dual_tool_package(workspace: &Path, id: &str) -> String {
        let manifest = format!(
            r#"
schema_version = 1
id = "{id}"
name = "{id}"
version = "0.1.0"
surfaces = ["tool"]
permissions = [{{ kind = "surface", surface = "tool" }}, {{ kind = "tool", name = "Echo" }}, {{ kind = "tool", name = "Other" }}]

[runtime]
kind = "wasm"
entry = "plugin.wasm"
abi = "yoi-plugin-wasm-1"

[[tools]]
name = "Echo"
description = "Echo input"
input_schema = {{ type = "object" }}

[[tools]]
name = "Other"
description = "Other input"
input_schema = {{ type = "object" }}
"#
        );
        write_plugin_manifest(workspace, id, &manifest)
    }

    fn write_plugin_manifest(workspace: &Path, id: &str, manifest: &str) -> String {
        let package_dir = workspace.join(".yoi/plugins");
        fs::create_dir_all(&package_dir).unwrap();
        let package = package_dir.join(format!("{id}.yoi-plugin"));
        write_stored_zip(
            &package,
            &[
                ("plugin.toml", manifest.as_bytes()),
                ("plugin.wasm", b"not wasm"),
            ],
        );

        let discovery = discover_plugins(&PluginDiscoveryOptions {
            workspace_root: workspace.to_path_buf(),
            user_data_home: None,
            limits: PluginDiscoveryLimits::default(),
        });
        discovery
            .packages
            .iter()
            .find(|package| package.identity.local_id == id)
            .unwrap()
            .digest
            .clone()
    }

    fn write_stored_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut bytes = Vec::new();
        let mut central = Vec::new();
        for (name, content) in entries {
            let local_offset = bytes.len() as u32;
            write_u32(&mut bytes, 0x0403_4b50);
            write_u16(&mut bytes, 20);
            write_u16(&mut bytes, 0x0800);
            write_u16(&mut bytes, 0);
            write_u16(&mut bytes, 0);
            write_u16(&mut bytes, 0);
            write_u32(&mut bytes, 0);
            write_u32(&mut bytes, content.len() as u32);
            write_u32(&mut bytes, content.len() as u32);
            write_u16(&mut bytes, name.len() as u16);
            write_u16(&mut bytes, 0);
            bytes.extend_from_slice(name.as_bytes());
            bytes.extend_from_slice(content);

            write_u32(&mut central, 0x0201_4b50);
            write_u16(&mut central, 20);
            write_u16(&mut central, 20);
            write_u16(&mut central, 0x0800);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u32(&mut central, 0);
            write_u32(&mut central, content.len() as u32);
            write_u32(&mut central, content.len() as u32);
            write_u16(&mut central, name.len() as u16);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u16(&mut central, 0);
            write_u32(&mut central, 0);
            write_u32(&mut central, local_offset);
            central.extend_from_slice(name.as_bytes());
        }
        let central_offset = bytes.len() as u32;
        bytes.extend_from_slice(&central);
        write_u32(&mut bytes, 0x0605_4b50);
        write_u16(&mut bytes, 0);
        write_u16(&mut bytes, 0);
        write_u16(&mut bytes, entries.len() as u16);
        write_u16(&mut bytes, entries.len() as u16);
        write_u32(&mut bytes, central.len() as u32);
        write_u32(&mut bytes, central_offset);
        write_u16(&mut bytes, 0);
        fs::write(path, bytes).unwrap();
    }

    fn write_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn write_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}
