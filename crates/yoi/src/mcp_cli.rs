use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use manifest::{
    McpConfig, McpEnvConfig, McpEnvValue, McpStdioCwdPolicy, McpStdioServerConfig,
    ProfileResolveOptions, ProfileResolver, ProfileSelector,
};
use serde::Serialize;

pub(crate) type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MAX_SERVERS: usize = 128;
const MAX_DIAGNOSTICS: usize = 48;
const MAX_TEXT_CHARS: usize = 240;
const MCP_STATIC_NOT_LIVE_REASON: &str = "CLI inspection reads resolved static MCP config only; provider-discovered state is unavailable without live Pod/runtime MCP state, and this command does not start MCP server processes.";

#[derive(Clone, Debug, Default)]
pub(crate) struct McpCliArgs {
    pub workspace: Option<PathBuf>,
    pub profile: Option<String>,
    pub json: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum McpCliCommand {
    List(McpCliArgs),
    Show {
        server: String,
        args: McpCliArgs,
    },
    Tools {
        server: Option<String>,
        args: McpCliArgs,
    },
    Resources {
        server: Option<String>,
        args: McpCliArgs,
    },
    Prompts {
        server: Option<String>,
        args: McpCliArgs,
    },
}

pub(crate) fn run(command: McpCliCommand) -> Result<()> {
    let rendered = match command {
        McpCliCommand::List(args) => render_list(&args)?,
        McpCliCommand::Show { server, args } => render_show(&server, &args)?,
        McpCliCommand::Tools { server, args } => render_tools(server.as_deref(), &args)?,
        McpCliCommand::Resources { server, args } => render_resources(server.as_deref(), &args)?,
        McpCliCommand::Prompts { server, args } => render_prompts(server.as_deref(), &args)?,
    };
    print!("{rendered}");
    Ok(())
}

fn render_list(args: &McpCliArgs) -> Result<String> {
    let snapshot = inspect_static_config(args);
    let report = ListReport::from_snapshot(snapshot);
    if args.json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(&report)?));
    }
    render_list_human(&report)
}

fn render_show(server: &str, args: &McpCliArgs) -> Result<String> {
    let snapshot = inspect_static_config(args);
    let report = ShowReport::from_snapshot(server, snapshot);
    if args.json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(&report)?));
    }
    render_show_human(&report)
}

fn render_tools(server: Option<&str>, args: &McpCliArgs) -> Result<String> {
    let snapshot = inspect_static_config(args);
    let report = ToolsReport::from_snapshot(server, snapshot);
    if args.json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(&report)?));
    }
    render_tools_human(&report)
}

fn render_resources(server: Option<&str>, args: &McpCliArgs) -> Result<String> {
    let snapshot = inspect_static_config(args);
    let report = ResourceLikeReport::from_snapshot(ResourceKind::Resources, server, snapshot);
    if args.json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(&report)?));
    }
    render_resource_like_human(&report)
}

fn render_prompts(server: Option<&str>, args: &McpCliArgs) -> Result<String> {
    let snapshot = inspect_static_config(args);
    let report = ResourceLikeReport::from_snapshot(ResourceKind::Prompts, server, snapshot);
    if args.json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(&report)?));
    }
    render_resource_like_human(&report)
}

fn inspect_static_config(args: &McpCliArgs) -> StaticConfigSnapshot {
    let workspace_input = match &args.workspace {
        Some(path) => path.clone(),
        None => match std::env::current_dir() {
            Ok(path) => path,
            Err(error) => {
                return StaticConfigSnapshot::invalid(
                    PathBuf::from("."),
                    profile_label(args),
                    DiagnosticReport::error(
                        "current_dir_unavailable",
                        format!("failed to resolve current directory: {error}"),
                    ),
                );
            }
        },
    };

    let workspace = match fs::canonicalize(&workspace_input) {
        Ok(path) => path,
        Err(error) => {
            return StaticConfigSnapshot::invalid(
                workspace_input.clone(),
                profile_label(args),
                DiagnosticReport::error(
                    "workspace_unavailable",
                    format!(
                        "workspace `{}` is unavailable: {error}",
                        workspace_input.display()
                    ),
                ),
            );
        }
    };

    let selector = args
        .profile
        .as_deref()
        .map(ProfileSelector::parse_cli)
        .unwrap_or(ProfileSelector::Default);
    let profile = selector.display_label();
    match ProfileResolver::new()
        .with_workspace_base(&workspace)
        .resolve(
            &selector,
            ProfileResolveOptions::with_pod_name("mcp-inspect"),
        ) {
        Ok(resolved) => {
            let mut diagnostics = vec![DiagnosticReport::info(
                "static_only",
                "static profile/config resolution completed; no MCP server process was started",
            )];
            diagnostics.push(DiagnosticReport::info(
                "live_state_unavailable",
                MCP_STATIC_NOT_LIVE_REASON,
            ));
            StaticConfigSnapshot {
                status: "ok".to_string(),
                workspace,
                profile,
                mcp: Some(resolved.manifest.mcp),
                diagnostics,
            }
        }
        Err(error) => StaticConfigSnapshot::invalid(
            workspace,
            profile,
            DiagnosticReport::error(
                "profile_resolution_failed",
                format!("failed to resolve profile/MCP config: {error}"),
            ),
        ),
    }
}

fn profile_label(args: &McpCliArgs) -> String {
    args.profile
        .clone()
        .unwrap_or_else(|| "default".to_string())
}

#[derive(Debug)]
struct StaticConfigSnapshot {
    status: String,
    workspace: PathBuf,
    profile: String,
    mcp: Option<McpConfig>,
    diagnostics: Vec<DiagnosticReport>,
}

impl StaticConfigSnapshot {
    fn invalid(workspace: PathBuf, profile: String, diagnostic: DiagnosticReport) -> Self {
        Self {
            status: "invalid_config".to_string(),
            workspace,
            profile,
            mcp: None,
            diagnostics: vec![
                diagnostic,
                DiagnosticReport::info("live_state_unavailable", MCP_STATIC_NOT_LIVE_REASON),
            ],
        }
    }
}

#[derive(Debug, Serialize)]
struct ListReport {
    command: &'static str,
    status: String,
    workspace: String,
    profile: String,
    inspection_mode: InspectionModeReport,
    live_state: LiveStateReport,
    summary: SummaryReport,
    servers: Vec<ServerReport>,
    diagnostics: Vec<DiagnosticReport>,
}

impl ListReport {
    fn from_snapshot(mut snapshot: StaticConfigSnapshot) -> Self {
        let (servers, mut diagnostics) = server_reports(snapshot.mcp.as_ref());
        diagnostics.splice(0..0, snapshot.diagnostics.drain(..).take(MAX_DIAGNOSTICS));
        diagnostics.truncate(MAX_DIAGNOSTICS);
        let summary = SummaryReport {
            configured_servers: servers.len(),
            truncated: snapshot
                .mcp
                .as_ref()
                .is_some_and(|mcp| mcp.stdio_servers.len() > MAX_SERVERS),
            provider_discovery: "not_live".to_string(),
        };
        Self {
            command: "list",
            status: snapshot.status,
            workspace: snapshot.workspace.display().to_string(),
            profile: snapshot.profile,
            inspection_mode: InspectionModeReport::static_config(),
            live_state: LiveStateReport::not_live(),
            summary,
            servers,
            diagnostics,
        }
    }
}

#[derive(Debug, Serialize)]
struct ShowReport {
    command: &'static str,
    status: String,
    workspace: String,
    profile: String,
    inspection_mode: InspectionModeReport,
    live_state: LiveStateReport,
    server: Option<ServerReport>,
    diagnostics: Vec<DiagnosticReport>,
}

impl ShowReport {
    fn from_snapshot(server_name: &str, snapshot: StaticConfigSnapshot) -> Self {
        let mut diagnostics = snapshot.diagnostics;
        let server = snapshot.mcp.as_ref().and_then(|mcp| {
            mcp.stdio_servers
                .iter()
                .find(|server| server.name == server_name)
                .map(ServerReport::from_stdio_server)
        });
        let status = if snapshot.status != "ok" {
            snapshot.status
        } else if server.is_some() {
            "ok".to_string()
        } else {
            diagnostics.push(DiagnosticReport::error(
                "server_not_configured",
                format!(
                    "MCP server `{}` is not configured",
                    bounded_text(server_name)
                ),
            ));
            "missing_server".to_string()
        };
        truncate_diagnostics(&mut diagnostics);
        Self {
            command: "show",
            status,
            workspace: snapshot.workspace.display().to_string(),
            profile: snapshot.profile,
            inspection_mode: InspectionModeReport::static_config(),
            live_state: LiveStateReport::not_live(),
            server,
            diagnostics,
        }
    }
}

#[derive(Debug, Serialize)]
struct ToolsReport {
    command: &'static str,
    status: String,
    workspace: String,
    profile: String,
    inspection_mode: InspectionModeReport,
    live_state: LiveStateReport,
    server_filter: Option<String>,
    summary: DiscoverySummaryReport,
    servers: Vec<DiscoveryServerReport>,
    tools: Vec<ToolItemReport>,
    diagnostics: Vec<DiagnosticReport>,
}

impl ToolsReport {
    fn from_snapshot(server_filter: Option<&str>, mut snapshot: StaticConfigSnapshot) -> Self {
        let (servers, mut diagnostics, missing) =
            discovery_servers(snapshot.mcp.as_ref(), server_filter, DiscoveryKind::Tools);
        diagnostics.splice(0..0, snapshot.diagnostics.drain(..).take(MAX_DIAGNOSTICS));
        let status = if snapshot.status != "ok" {
            snapshot.status
        } else if missing {
            "missing_server".to_string()
        } else {
            "not_live".to_string()
        };
        truncate_diagnostics(&mut diagnostics);
        Self {
            command: "tools",
            status,
            workspace: snapshot.workspace.display().to_string(),
            profile: snapshot.profile,
            inspection_mode: InspectionModeReport::static_config(),
            live_state: LiveStateReport::not_live(),
            server_filter: server_filter.map(ToString::to_string),
            summary: DiscoverySummaryReport {
                configured_servers: servers.len(),
                discovered_items: 0,
                discovery_status: "unavailable".to_string(),
                reason: MCP_STATIC_NOT_LIVE_REASON.to_string(),
            },
            servers,
            tools: Vec::new(),
            diagnostics,
        }
    }
}

#[derive(Debug, Serialize)]
struct ResourceLikeReport {
    command: &'static str,
    status: String,
    workspace: String,
    profile: String,
    inspection_mode: InspectionModeReport,
    live_state: LiveStateReport,
    server_filter: Option<String>,
    summary: DiscoverySummaryReport,
    servers: Vec<DiscoveryServerReport>,
    operations: Vec<OperationReport>,
    items: Vec<ResourceItemReport>,
    diagnostics: Vec<DiagnosticReport>,
}

impl ResourceLikeReport {
    fn from_snapshot(
        kind: ResourceKind,
        server_filter: Option<&str>,
        mut snapshot: StaticConfigSnapshot,
    ) -> Self {
        let (servers, mut diagnostics, missing) =
            discovery_servers(snapshot.mcp.as_ref(), server_filter, kind.discovery_kind());
        diagnostics.splice(0..0, snapshot.diagnostics.drain(..).take(MAX_DIAGNOSTICS));
        let status = if snapshot.status != "ok" {
            snapshot.status
        } else if missing {
            "missing_server".to_string()
        } else {
            "not_live".to_string()
        };
        let operations = servers
            .iter()
            .flat_map(|server| operation_reports_for_server(kind, &server.server_name))
            .collect::<Vec<_>>();
        truncate_diagnostics(&mut diagnostics);
        Self {
            command: kind.command_name(),
            status,
            workspace: snapshot.workspace.display().to_string(),
            profile: snapshot.profile,
            inspection_mode: InspectionModeReport::static_config(),
            live_state: LiveStateReport::not_live(),
            server_filter: server_filter.map(ToString::to_string),
            summary: DiscoverySummaryReport {
                configured_servers: servers.len(),
                discovered_items: 0,
                discovery_status: "unavailable".to_string(),
                reason: MCP_STATIC_NOT_LIVE_REASON.to_string(),
            },
            servers,
            operations,
            items: Vec::new(),
            diagnostics,
        }
    }
}

#[derive(Debug, Serialize)]
struct InspectionModeReport {
    kind: &'static str,
    starts_servers: bool,
    calls_tools: bool,
    fetches_resource_or_prompt_content: bool,
}

impl InspectionModeReport {
    fn static_config() -> Self {
        Self {
            kind: "static_config",
            starts_servers: false,
            calls_tools: false,
            fetches_resource_or_prompt_content: false,
        }
    }
}

#[derive(Debug, Serialize)]
struct LiveStateReport {
    status: &'static str,
    available: bool,
    reason: &'static str,
}

impl LiveStateReport {
    fn not_live() -> Self {
        Self {
            status: "not_live",
            available: false,
            reason: MCP_STATIC_NOT_LIVE_REASON,
        }
    }
}

#[derive(Debug, Serialize)]
struct SummaryReport {
    configured_servers: usize,
    truncated: bool,
    provider_discovery: String,
}

#[derive(Debug, Clone, Serialize)]
struct ServerReport {
    name: String,
    status: &'static str,
    transport: TransportReport,
    cwd: CwdReport,
    env_policy: EnvPolicyReport,
    trust_policy: TrustPolicyReport,
    capabilities: CapabilitySummaryReport,
    diagnostics: Vec<DiagnosticReport>,
}

impl ServerReport {
    fn from_stdio_server(server: &McpStdioServerConfig) -> Self {
        Self {
            name: bounded_text(&server.name),
            status: "configured",
            transport: TransportReport {
                kind: "stdio",
                command: bounded_text(&server.command),
                arg_count: server.args.len(),
                args_redacted: true,
            },
            cwd: CwdReport::from_policy(server.cwd.as_ref()),
            env_policy: EnvPolicyReport::from_env(&server.env),
            trust_policy: TrustPolicyReport::read_only_inspection(),
            capabilities: CapabilitySummaryReport::not_live(),
            diagnostics: vec![DiagnosticReport::info(
                "provider_discovery_unavailable",
                MCP_STATIC_NOT_LIVE_REASON,
            )],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct TransportReport {
    kind: &'static str,
    command: String,
    arg_count: usize,
    args_redacted: bool,
}

#[derive(Debug, Clone, Serialize)]
struct CwdReport {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

impl CwdReport {
    fn from_policy(policy: Option<&McpStdioCwdPolicy>) -> Self {
        match policy {
            Some(McpStdioCwdPolicy::Inherit) => Self {
                kind: "inherit",
                path: None,
            },
            Some(McpStdioCwdPolicy::Path { path }) => Self {
                kind: "path",
                path: Some(path.display().to_string()),
            },
            None => Self {
                kind: "unspecified",
                path: None,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct EnvPolicyReport {
    inherit_count: usize,
    set_count: usize,
    value_kinds: EnvValueKindCounts,
    values_redacted: bool,
}

impl EnvPolicyReport {
    fn from_env(env: &McpEnvConfig) -> Self {
        let mut value_kinds = EnvValueKindCounts::default();
        for value in env.set.values() {
            match value {
                McpEnvValue::Literal { .. } => value_kinds.literal += 1,
                McpEnvValue::SecretRef { .. } => value_kinds.secret_ref += 1,
                McpEnvValue::EnvRef { .. } => value_kinds.env_ref += 1,
            }
        }
        Self {
            inherit_count: env.inherit.len(),
            set_count: env.set.len(),
            value_kinds,
            values_redacted: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
struct EnvValueKindCounts {
    literal: usize,
    secret_ref: usize,
    env_ref: usize,
}

#[derive(Debug, Clone, Serialize)]
struct TrustPolicyReport {
    process_start: &'static str,
    tool_calls: &'static str,
    resource_prompt_content: &'static str,
    server_metadata: &'static str,
}

impl TrustPolicyReport {
    fn read_only_inspection() -> Self {
        Self {
            process_start: "not_started_by_cli_inspection",
            tool_calls: "not_called_by_cli_inspection",
            resource_prompt_content: "not_fetched_by_cli_inspection",
            server_metadata: "untrusted_when_available",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct CapabilitySummaryReport {
    tools: &'static str,
    resources: &'static str,
    prompts: &'static str,
    diagnostics: Vec<DiagnosticReport>,
}

impl CapabilitySummaryReport {
    fn not_live() -> Self {
        Self {
            tools: "unavailable_not_live",
            resources: "unavailable_not_live",
            prompts: "unavailable_not_live",
            diagnostics: vec![DiagnosticReport::info(
                "capabilities_require_live_initialize",
                "MCP capabilities are server-provided during initialize and are unavailable in static config inspection",
            )],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct DiagnosticReport {
    level: &'static str,
    code: &'static str,
    message: String,
}

impl DiagnosticReport {
    fn info(code: &'static str, message: impl AsRef<str>) -> Self {
        Self {
            level: "info",
            code,
            message: bounded_text(message.as_ref()),
        }
    }
    fn warning(code: &'static str, message: impl AsRef<str>) -> Self {
        Self {
            level: "warning",
            code,
            message: bounded_text(message.as_ref()),
        }
    }
    fn error(code: &'static str, message: impl AsRef<str>) -> Self {
        Self {
            level: "error",
            code,
            message: bounded_text(message.as_ref()),
        }
    }
}

#[derive(Debug, Serialize)]
struct DiscoverySummaryReport {
    configured_servers: usize,
    discovered_items: usize,
    discovery_status: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct DiscoveryServerReport {
    server_name: String,
    transport_kind: &'static str,
    capability_status: &'static str,
    discovery_status: &'static str,
    registration_status: &'static str,
    diagnostics: Vec<DiagnosticReport>,
}

#[derive(Debug, Serialize)]
struct ToolItemReport {
    yoi_stable_tool_name: Option<String>,
    server_name: String,
    mcp_tool_name: Option<String>,
    schema_availability: &'static str,
    registration_status: &'static str,
    diagnostics: Vec<DiagnosticReport>,
}

#[derive(Debug, Serialize)]
struct ResourceItemReport {
    server_name: String,
    kind: &'static str,
    identifier: Option<String>,
    name: Option<String>,
    description: Option<String>,
    eligibility: &'static str,
    content_state: &'static str,
    diagnostics: Vec<DiagnosticReport>,
}

#[derive(Debug, Serialize)]
struct OperationReport {
    server_name: String,
    method: &'static str,
    yoi_stable_tool_name: String,
    registration_status: &'static str,
    eligibility: &'static str,
    content_state: &'static str,
    diagnostics: Vec<DiagnosticReport>,
}

#[derive(Clone, Copy)]
enum DiscoveryKind {
    Tools,
    Resources,
    Prompts,
}

impl DiscoveryKind {
    fn label(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Resources => "resources",
            Self::Prompts => "prompts",
        }
    }
}

#[derive(Clone, Copy)]
enum ResourceKind {
    Resources,
    Prompts,
}

impl ResourceKind {
    fn command_name(self) -> &'static str {
        match self {
            Self::Resources => "resources",
            Self::Prompts => "prompts",
        }
    }
    fn discovery_kind(self) -> DiscoveryKind {
        match self {
            Self::Resources => DiscoveryKind::Resources,
            Self::Prompts => DiscoveryKind::Prompts,
        }
    }
}

fn server_reports(config: Option<&McpConfig>) -> (Vec<ServerReport>, Vec<DiagnosticReport>) {
    let Some(config) = config else {
        return (Vec::new(), Vec::new());
    };
    let mut diagnostics = Vec::new();
    if config.stdio_servers.len() > MAX_SERVERS {
        diagnostics.push(DiagnosticReport::warning(
            "server_list_truncated",
            format!(
                "configured MCP server list truncated from {} to {} entries",
                config.stdio_servers.len(),
                MAX_SERVERS
            ),
        ));
    }
    let servers = config
        .stdio_servers
        .iter()
        .take(MAX_SERVERS)
        .map(ServerReport::from_stdio_server)
        .collect();
    (servers, diagnostics)
}

fn selected_stdio_servers<'a>(
    config: Option<&'a McpConfig>,
    server_filter: Option<&str>,
) -> (Vec<&'a McpStdioServerConfig>, bool) {
    let Some(config) = config else {
        return (Vec::new(), false);
    };
    match server_filter {
        Some(filter) => {
            let servers = config
                .stdio_servers
                .iter()
                .filter(|server| server.name == filter)
                .collect::<Vec<_>>();
            let missing = servers.is_empty();
            (servers, missing)
        }
        None => (
            config.stdio_servers.iter().take(MAX_SERVERS).collect(),
            false,
        ),
    }
}

fn discovery_servers(
    config: Option<&McpConfig>,
    server_filter: Option<&str>,
    kind: DiscoveryKind,
) -> (Vec<DiscoveryServerReport>, Vec<DiagnosticReport>, bool) {
    let (servers, missing) = selected_stdio_servers(config, server_filter);
    let mut diagnostics = Vec::new();
    if missing {
        if let Some(filter) = server_filter {
            diagnostics.push(DiagnosticReport::error(
                "server_not_configured",
                format!("MCP server `{}` is not configured", bounded_text(filter)),
            ));
        }
    } else if let Some(config) =
        config.filter(|config| server_filter.is_none() && config.stdio_servers.len() > MAX_SERVERS)
    {
        diagnostics.push(DiagnosticReport::warning(
            "server_list_truncated",
            format!(
                "configured MCP server list truncated from {} to {} entries",
                config.stdio_servers.len(),
                MAX_SERVERS
            ),
        ));
    }

    let reports = servers
        .into_iter()
        .map(|server| DiscoveryServerReport {
            server_name: bounded_text(&server.name),
            transport_kind: "stdio",
            capability_status: "unavailable_not_live",
            discovery_status: "unavailable",
            registration_status: "not_live",
            diagnostics: vec![DiagnosticReport::info(
                "provider_discovery_unavailable",
                format!(
                    "MCP {} discovery for server `{}` is unavailable in static CLI inspection; no server process was started",
                    kind.label(),
                    bounded_text(&server.name)
                ),
            )],
        })
        .collect();
    (reports, diagnostics, missing)
}

fn operation_reports_for_server(kind: ResourceKind, server_name: &str) -> Vec<OperationReport> {
    let operations: &[(&str, &str)] = match kind {
        ResourceKind::Resources => &[
            ("resources/list", "resources_list"),
            ("resources/read", "resources_read"),
        ],
        ResourceKind::Prompts => &[
            ("prompts/list", "prompts_list"),
            ("prompts/get", "prompts_get"),
        ],
    };
    let server_namespace = sanitize_segment(server_name);
    operations
        .iter()
        .map(|(method, segment)| OperationReport {
            server_name: server_name.to_string(),
            method,
            yoi_stable_tool_name: format!("Mcp_{server_namespace}_{segment}"),
            registration_status: "not_live",
            eligibility: "unknown_until_live_capability_discovery",
            content_state: "not_fetched",
            diagnostics: vec![DiagnosticReport::info(
                "operation_requires_live_capability",
                format!(
                    "{} operation registration requires server capability discovery; CLI inspection did not initialize the server",
                    method
                ),
            )],
        })
        .collect()
}

fn render_list_human(report: &ListReport) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "mcp servers (workspace: {}, profile: {})",
        report.workspace, report.profile
    )?;
    writeln!(out, "  status: {}", report.status)?;
    writeln!(out, "  mode: static config inspection")?;
    writeln!(out, "  live: not live / unavailable")?;
    if report.servers.is_empty() {
        if report.status == "ok" {
            writeln!(out, "  no MCP servers configured")?;
        } else {
            writeln!(out, "  MCP server config unavailable or invalid")?;
        }
    } else {
        for server in &report.servers {
            writeln!(out, "  - {} [{}]", server.name, server.transport.kind)?;
            writeln!(out, "      command: {}", server.transport.command)?;
            writeln!(out, "      args: {} redacted", server.transport.arg_count)?;
            writeln!(out, "      cwd: {}", cwd_human(&server.cwd))?;
            writeln!(
                out,
                "      env: inherit_count={} set_count={} values=redacted",
                server.env_policy.inherit_count, server.env_policy.set_count
            )?;
            writeln!(out, "      capabilities: unavailable (not live)")?;
        }
    }
    render_diagnostics_human(&mut out, &report.diagnostics)?;
    Ok(out)
}

fn render_show_human(report: &ShowReport) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "mcp server (workspace: {}, profile: {})",
        report.workspace, report.profile
    )?;
    writeln!(out, "  status: {}", report.status)?;
    writeln!(out, "  live: not live / unavailable")?;
    match &report.server {
        Some(server) => {
            writeln!(out, "  name: {}", server.name)?;
            writeln!(out, "  transport: {}", server.transport.kind)?;
            writeln!(out, "  command: {}", server.transport.command)?;
            writeln!(out, "  args: {} redacted", server.transport.arg_count)?;
            writeln!(out, "  cwd: {}", cwd_human(&server.cwd))?;
            writeln!(
                out,
                "  env: inherit_count={} set_count={} values=redacted",
                server.env_policy.inherit_count, server.env_policy.set_count
            )?;
            writeln!(out, "  trust:")?;
            writeln!(
                out,
                "    process_start: {}",
                server.trust_policy.process_start
            )?;
            writeln!(out, "    tool_calls: {}", server.trust_policy.tool_calls)?;
            writeln!(
                out,
                "    resource_prompt_content: {}",
                server.trust_policy.resource_prompt_content
            )?;
            writeln!(out, "  capabilities: unavailable (not live)")?;
        }
        None => {
            writeln!(out, "  server: missing or invalid")?;
        }
    }
    render_diagnostics_human(&mut out, &report.diagnostics)?;
    Ok(out)
}

fn render_tools_human(report: &ToolsReport) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "mcp tools (workspace: {}, profile: {})",
        report.workspace, report.profile
    )?;
    writeln!(out, "  status: {}", report.status)?;
    writeln!(out, "  live: not live / unavailable")?;
    if let Some(filter) = &report.server_filter {
        writeln!(out, "  server filter: {filter}")?;
    }
    if report.tools.is_empty() {
        writeln!(out, "  provider-discovered tools: unavailable (not live)")?;
    }
    for server in &report.servers {
        writeln!(
            out,
            "  - server {}: discovery={}, registration={}",
            server.server_name, server.discovery_status, server.registration_status
        )?;
    }
    render_diagnostics_human(&mut out, &report.diagnostics)?;
    Ok(out)
}

fn render_resource_like_human(report: &ResourceLikeReport) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "mcp {} (workspace: {}, profile: {})",
        report.command, report.workspace, report.profile
    )?;
    writeln!(out, "  status: {}", report.status)?;
    writeln!(out, "  live: not live / unavailable")?;
    if let Some(filter) = &report.server_filter {
        writeln!(out, "  server filter: {filter}")?;
    }
    if report.items.is_empty() {
        writeln!(
            out,
            "  provider-discovered {}: unavailable (not live); content not fetched",
            report.command
        )?;
    }
    for server in &report.servers {
        writeln!(
            out,
            "  - server {}: discovery={}, registration={}",
            server.server_name, server.discovery_status, server.registration_status
        )?;
    }
    if !report.operations.is_empty() {
        writeln!(
            out,
            "  explicit operation tools (names are static; eligibility unknown until live discovery):"
        )?;
        for op in &report.operations {
            writeln!(
                out,
                "    - {} -> {} ({}, content={})",
                op.method, op.yoi_stable_tool_name, op.registration_status, op.content_state
            )?;
        }
    }
    render_diagnostics_human(&mut out, &report.diagnostics)?;
    Ok(out)
}

fn cwd_human(cwd: &CwdReport) -> String {
    match &cwd.path {
        Some(path) => format!("{} ({path})", cwd.kind),
        None => cwd.kind.to_string(),
    }
}

fn render_diagnostics_human(out: &mut String, diagnostics: &[DiagnosticReport]) -> Result<()> {
    if diagnostics.is_empty() {
        return Ok(());
    }
    writeln!(out, "  diagnostics:")?;
    for diagnostic in diagnostics.iter().take(MAX_DIAGNOSTICS) {
        writeln!(
            out,
            "    - {} [{}]: {}",
            diagnostic.level, diagnostic.code, diagnostic.message
        )?;
    }
    Ok(())
}

fn truncate_diagnostics(diagnostics: &mut Vec<DiagnosticReport>) {
    diagnostics.truncate(MAX_DIAGNOSTICS);
}

fn sanitize_segment(input: &str) -> String {
    let mut output = String::new();
    let mut last_underscore = false;
    for ch in input.chars() {
        let normalized = if ch.is_ascii_alphanumeric() { ch } else { '_' };
        if normalized == '_' {
            if last_underscore {
                continue;
            }
            last_underscore = true;
        } else {
            last_underscore = false;
        }
        output.push(normalized);
        if output.len() >= 48 {
            break;
        }
    }
    let output = output.trim_matches('_').to_string();
    if output.is_empty() {
        "unnamed".to_string()
    } else {
        output
    }
}

fn bounded_text(input: &str) -> String {
    let mut out = String::new();
    let mut previous_space = false;
    for ch in input.chars() {
        let normalized = if ch.is_control() { ' ' } else { ch };
        if normalized.is_whitespace() {
            if previous_space {
                continue;
            }
            previous_space = true;
            out.push(' ');
        } else {
            previous_space = false;
            out.push(normalized);
        }
        if out.chars().count() >= MAX_TEXT_CHARS {
            out.push('…');
            break;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_profile(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn workspace_and_profile(body: &str) -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let profile = write_profile(tmp.path(), "mcp.lua", body);
        (tmp, workspace, profile)
    }

    fn mcp_profile() -> String {
        r#"
local profile = require("yoi.profile")
return profile {
  slug = "mcp-test",
  model = { scheme = "anthropic", model_id = "claude-sonnet-4-20250514" },
  mcp = {
    stdio_server = {
      {
        name = "filesystem",
        command = "definitely-not-spawned-during-cli-inspection",
        args = { "--root", ".", "--token", "ARG_SECRET_DO_NOT_PRINT" },
        env = {
          inherit = { "PATH" },
          set = {
            SAFE_MODE = { kind = "literal", value = "SUPER_SECRET_LITERAL_DO_NOT_PRINT" },
            API_TOKEN = { kind = "secret_ref", ref = "providers/mcp-token" },
            FROM_ENV = { kind = "env_ref", name = "SECRET_ENV_NAME_DO_NOT_PRINT" },
          },
        },
      },
    },
  },
}
"#
        .to_string()
    }

    fn args(workspace: &Path, profile: &Path, json: bool) -> McpCliArgs {
        McpCliArgs {
            workspace: Some(workspace.to_path_buf()),
            profile: Some(format!("path:{}", profile.display())),
            json,
        }
    }

    #[test]
    fn list_json_reports_resolved_servers_without_secret_values_or_args() {
        let (_tmp, workspace, profile) = workspace_and_profile(&mcp_profile());
        let output = render_list(&args(&workspace, &profile, true)).unwrap();
        assert!(output.contains("filesystem"));
        assert!(output.contains("definitely-not-spawned-during-cli-inspection"));
        assert!(!output.contains("ARG_SECRET_DO_NOT_PRINT"));
        assert!(!output.contains("SUPER_SECRET_LITERAL_DO_NOT_PRINT"));
        assert!(!output.contains("providers/mcp-token"));
        assert!(!output.contains("SECRET_ENV_NAME_DO_NOT_PRINT"));

        let json: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(json["command"], "list");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["servers"][0]["transport"]["kind"], "stdio");
        assert_eq!(json["servers"][0]["transport"]["arg_count"], 4);
        assert_eq!(json["servers"][0]["env_policy"]["set_count"], 3);
        assert_eq!(json["live_state"]["status"], "not_live");
        assert_eq!(json["inspection_mode"]["starts_servers"], false);
    }

    #[test]
    fn show_json_reports_missing_server_explicitly() {
        let (_tmp, workspace, profile) = workspace_and_profile(&mcp_profile());
        let output = render_show("missing", &args(&workspace, &profile, true)).unwrap();
        let json: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(json["command"], "show");
        assert_eq!(json["status"], "missing_server");
        assert!(json["server"].is_null());
        assert!(output.contains("server_not_configured"));
    }

    #[test]
    fn tools_json_reports_provider_discovery_unavailable_not_stale() {
        let (_tmp, workspace, profile) = workspace_and_profile(&mcp_profile());
        let output = render_tools(None, &args(&workspace, &profile, true)).unwrap();
        let json: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(json["command"], "tools");
        assert_eq!(json["status"], "not_live");
        assert_eq!(json["tools"].as_array().unwrap().len(), 0);
        assert_eq!(json["servers"][0]["discovery_status"], "unavailable");
        assert_eq!(json["servers"][0]["registration_status"], "not_live");
        assert!(output.contains("no server process was started"));
    }

    #[test]
    fn resources_and_prompts_json_report_operation_eligibility_without_content() {
        let (_tmp, workspace, profile) = workspace_and_profile(&mcp_profile());
        let resource_output =
            render_resources(Some("filesystem"), &args(&workspace, &profile, true)).unwrap();
        let resources: Value = serde_json::from_str(&resource_output).unwrap();
        assert_eq!(resources["command"], "resources");
        assert_eq!(resources["status"], "not_live");
        assert_eq!(resources["items"].as_array().unwrap().len(), 0);
        assert_eq!(
            resources["operations"][0]["yoi_stable_tool_name"],
            "Mcp_filesystem_resources_list"
        );
        assert_eq!(resources["operations"][0]["content_state"], "not_fetched");
        assert!(!resource_output.contains("RESOURCE_CONTENT_DO_NOT_PRINT"));

        let prompt_output =
            render_prompts(Some("filesystem"), &args(&workspace, &profile, true)).unwrap();
        let prompts: Value = serde_json::from_str(&prompt_output).unwrap();
        assert_eq!(prompts["command"], "prompts");
        assert_eq!(prompts["status"], "not_live");
        assert_eq!(prompts["items"].as_array().unwrap().len(), 0);
        assert_eq!(
            prompts["operations"][1]["yoi_stable_tool_name"],
            "Mcp_filesystem_prompts_get"
        );
        assert_eq!(prompts["operations"][1]["content_state"], "not_fetched");
        assert!(!prompt_output.contains("PROMPT_CONTENT_DO_NOT_PRINT"));
    }

    #[test]
    fn invalid_config_is_reported_as_invalid_not_silently_empty() {
        let body = r#"
local profile = require("yoi.profile")
return profile {
  slug = "bad-mcp",
  model = { scheme = "anthropic", model_id = "claude-sonnet-4-20250514" },
  mcp = {
    stdio_server = {
      { name = "dup", command = "one" },
      { name = "dup", command = "two" },
    },
  },
}
"#;
        let (_tmp, workspace, profile) = workspace_and_profile(body);
        let output = render_list(&args(&workspace, &profile, true)).unwrap();
        let json: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(json["status"], "invalid_config");
        assert_eq!(json["servers"].as_array().unwrap().len(), 0);
        assert!(output.contains("profile_resolution_failed"));
        assert!(output.contains("duplicate stdio server name"));
    }

    #[test]
    fn human_output_distinguishes_empty_and_unavailable() {
        let body = r#"
local profile = require("yoi.profile")
return profile {
  slug = "empty-mcp",
  model = { scheme = "anthropic", model_id = "claude-sonnet-4-20250514" },
}
"#;
        let (_tmp, workspace, profile) = workspace_and_profile(body);
        let output = render_list(&args(&workspace, &profile, false)).unwrap();
        assert!(output.contains("no MCP servers configured"));
        assert!(output.contains("live: not live / unavailable"));
    }
}
