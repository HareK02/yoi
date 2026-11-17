//! System prompt template machinery for the Worker layer.
//!
//! Manifests describe the system prompt body as a reference to a
//! prompt asset (`worker.instruction`, see [`manifest::EngineManifest`]).
//! [`SystemPromptTemplate`] resolves that reference through a
//! [`PromptCatalogSource`], parses the source as a minijinja template, and
//! eagerly syntax-checks it at Worker construction. The final system
//! prompt is materialised exactly once just before the first LLM turn:
//! the rendered body is appended with a fixed trailing section carrying
//! the Worker's `Scope` summary, actual Workdir session environments, (if
//! present) the project's `AGENTS.md` contents, resident memory sections, and
//! conditional Worker-orchestration guidance, then the whole string is handed
//! to the Engine via `set_system_prompt`. Subsequent turns and compactions reuse that
//! materialised string verbatim.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use manifest::Scope;
use minijinja::value::Value;
use thiserror::Error;

use crate::feature::{FeatureInstructionDeclaration, dedupe_instruction_contributions};
use crate::prompt::catalog::{CatalogError, PromptCatalog};
use crate::prompt::source::PromptCatalogSource;

#[derive(Debug, Error)]
pub enum SystemPromptError {
    #[error("system prompt template parse error: {0}")]
    Parse(String),
    #[error("system prompt template render error: {0}")]
    Render(String),
    #[error("failed to render trailing section template: {0}")]
    Catalog(#[from] CatalogError),
}

/// Parsed instruction template bound to one immutable effective Prompt catalog.
#[derive(Clone)]
pub struct SystemPromptTemplate {
    catalog: Arc<PromptCatalog>,
    instruction_name: String,
}

impl SystemPromptTemplate {
    /// Resolve an exact catalog-root dotted Prompt name and eagerly verify it.
    pub fn parse(
        instruction_ref: &str,
        loader: PromptCatalogSource,
    ) -> Result<Self, SystemPromptError> {
        let instruction_name = exact_prompt_name(instruction_ref).ok_or_else(|| {
            SystemPromptError::Parse(format!(
                "instruction must be an exact catalog-root dotted Prompt name: {instruction_ref}"
            ))
        })?;
        let catalog = if let Some(projection) = loader.effective_catalog() {
            Arc::new(PromptCatalog::from_projection(projection.clone())?)
        } else {
            PromptCatalog::builtins_only()?
        };
        if !catalog.contains(&instruction_name) {
            return Err(SystemPromptError::Parse(format!(
                "Prompt '{instruction_name}' is not present in the effective catalog"
            )));
        }
        Ok(Self {
            catalog,
            instruction_name,
        })
    }

    /// Render the instruction body and append the fixed trailing
    /// section (scope summary + optional AGENTS.md). The trailing
    /// section is assembled in Rust so that authored templates cannot
    /// accidentally omit the scope boundary or the project instructions.
    pub fn render(&self, ctx: &SystemPromptContext<'_>) -> Result<String, SystemPromptError> {
        let body = self
            .catalog
            .render_name(&self.instruction_name, ctx.to_minijinja_value())
            .map_err(|error| SystemPromptError::Render(error.to_string()))?;
        append_trailing_section(
            &body,
            ctx,
            &self.catalog,
            ctx.scope,
            ctx.agents_md.as_deref(),
            ctx.resident_summary,
        )
    }
}

impl std::fmt::Debug for SystemPromptTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemPromptTemplate")
            .field("instruction", &self.instruction_name)
            .finish_non_exhaustive()
    }
}

/// Inputs available to an instruction template at materialisation time.
///
/// Scope summary and AGENTS.md are deliberately **not** exposed to the
/// template — they live in the Rust-owned trailing section so user
/// templates cannot drop them on the floor.
pub struct SystemPromptContext<'a> {
    pub now: DateTime<Utc>,
    /// The same session router used by the Worker's filesystem tools.
    pub workdir_sessions: &'a workdir::WorkdirSessionRouter,
    /// Language policy exposed to instruction templates as `{{ language }}`.
    pub language: &'a str,
    pub scope: &'a Scope,
    pub tool_names: Vec<String>,
    pub feature_instructions: &'a [FeatureInstructionDeclaration],
    /// Project-level instructions read from the nearest `AGENTS.md`.
    /// Not visible from the template; consumed by the trailing-section
    /// formatter in [`SystemPromptTemplate::render`].
    pub agents_md: Option<String>,
    /// The body of the workspace Memory document. `None` disables the
    /// resident summary section; empty strings are ignored by the trailing-section
    /// formatter.
    pub resident_summary: Option<&'a str>,
    /// Catalog used to render the fixed trailing section headers.
    /// Passed by reference so callers do not give up ownership across
    /// the short-lived render borrow.
    pub prompts: &'a PromptCatalog,
}

impl<'a> SystemPromptContext<'a> {
    fn to_minijinja_value(&self) -> Value {
        let mut root: BTreeMap<String, Value> = BTreeMap::new();
        root.insert(
            "date".into(),
            Value::from(self.now.format("%Y-%m-%d").to_string()),
        );
        root.insert(
            "time".into(),
            Value::from(self.now.format("%H:%M:%S").to_string()),
        );
        root.insert(
            "datetime".into(),
            Value::from(self.now.to_rfc3339_opts(SecondsFormat::Secs, true)),
        );
        // A singular template cwd is meaningful only with one attachment. Never
        // substitute the local Profile/Memory discovery directory for a session cwd.
        let cwd = self
            .workdir_sessions
            .only_session()
            .map(|session| display_workdir_path(&session.environment().cwd).to_owned())
            .unwrap_or_else(|| "no single Workdir session cwd; see execution environment".into());
        root.insert("cwd".into(), Value::from(cwd));
        root.insert("language".into(), Value::from(self.language));
        root.insert(
            "tools".into(),
            Value::from(
                self.tool_names
                    .iter()
                    .cloned()
                    .map(Value::from)
                    .collect::<Vec<_>>(),
            ),
        );
        root.insert(
            "tool_capabilities".into(),
            ToolCapabilities::from_tool_names(&self.tool_names).to_minijinja_value(),
        );
        Value::from(root)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ToolCapabilities {
    memory_query: bool,
    memory_read_document: bool,
    memory_update_document: bool,
    sub_worker_spawn: bool,
    sub_worker_send: bool,
    sub_worker_stop: bool,
    sub_worker_list: bool,
    sub_worker_restore: bool,
}

impl ToolCapabilities {
    fn from_tool_names(names: &[String]) -> Self {
        let mut capabilities = Self::default();
        for name in names {
            match name.as_str() {
                "MemoryQuery" => capabilities.memory_query = true,
                "MemoryReadDocument" => capabilities.memory_read_document = true,
                "MemoryUpdateDocument" => capabilities.memory_update_document = true,
                "SubWorkerSpawn" => capabilities.sub_worker_spawn = true,
                _ => {}
            }
        }
        if capabilities.sub_worker_spawn {
            for name in names {
                match name.as_str() {
                    "WorkerSendInput" => capabilities.sub_worker_send = true,
                    "WorkerStop" => capabilities.sub_worker_stop = true,
                    "WorkerList" => capabilities.sub_worker_list = true,
                    _ => {}
                }
            }
        }
        capabilities
    }

    fn memory_records(self) -> bool {
        self.memory_query || self.memory_read_document || self.memory_update_document
    }

    fn memory_any(self) -> bool {
        self.memory_records()
    }

    fn memory_mutation(self) -> bool {
        self.memory_update_document
    }

    fn sub_worker_management(self) -> bool {
        self.sub_worker_spawn
            || self.sub_worker_send
            || self.sub_worker_stop
            || self.sub_worker_list
            || self.sub_worker_restore
    }

    fn to_minijinja_value(self) -> Value {
        let mut map: BTreeMap<&'static str, Value> = BTreeMap::new();
        map.insert("memory_any", Value::from(self.memory_any()));
        map.insert("memory_records", Value::from(self.memory_records()));
        map.insert("memory_query", Value::from(self.memory_query));
        map.insert(
            "memory_read_document",
            Value::from(self.memory_read_document),
        );
        map.insert(
            "memory_update_document",
            Value::from(self.memory_update_document),
        );
        map.insert("memory_mutation", Value::from(self.memory_mutation()));
        map.insert(
            "sub_worker_management",
            Value::from(self.sub_worker_management()),
        );
        Value::from(map)
    }
}

fn exact_prompt_name(reference: &str) -> Option<String> {
    let candidate = reference.to_string();
    if candidate.is_empty()
        || candidate.split('.').any(|segment| {
            segment.is_empty()
                || !segment.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
                })
        })
    {
        None
    } else {
        Some(candidate)
    }
}

fn display_workdir_path(path: &workdir::WorkdirPath) -> &str {
    if path.as_str().is_empty() {
        "."
    } else {
        path.as_str()
    }
}

/// Project provider-owned session metadata, rather than reconstructing authority
/// from a manifest Scope or the directory used to discover Profiles and Memory.
pub(crate) fn workdir_environment_section(
    router: &workdir::WorkdirSessionRouter,
) -> Result<String, SystemPromptError> {
    use workdir::WorkdirSessionCapability::{Command, Edit, Glob, Grep, Read, Write};

    let mut environments = Vec::new();
    for alias in router.aliases() {
        let resolved = router
            .resolve(Some(alias.as_str()))
            .map_err(|error| SystemPromptError::Render(error.to_string()))?;
        let environment = resolved.session.environment();
        let capabilities: Vec<_> = [
            (Read, "read"),
            (Write, "write"),
            (Edit, "edit"),
            (Glob, "glob"),
            (Grep, "grep"),
            (Command, "command"),
        ]
        .into_iter()
        .filter_map(|(capability, name)| {
            environment
                .capabilities
                .supports(capability)
                .then_some(name)
        })
        .collect();
        environments.push(serde_json::json!({
            "alias": alias.as_str(),
            "cwd": display_workdir_path(&environment.cwd),
            "scope": environment.scope,
            "restrictions": environment.restrictions,
            "capabilities": capabilities,
            "command": environment.capabilities.supports(Command),
        }));
    }
    let json = serde_json::to_string_pretty(&environments)
        .map_err(|error| SystemPromptError::Render(error.to_string()))?;
    Ok(format!(
        "## Workdir execution environment\n\n\
         Select a Workdir by its alias (`target_workdir`); omission is allowed only with one attachment. \
         Relative filesystem operation paths are resolved from that selected session's cwd, not its Workdir root. \
         Paths returned by Glob/Grep use the same cwd-relative coordinates and can be passed unchanged to other filesystem tools. \
         Bash starts at the session cwd; an explicit relative cwd is resolved from it, and shell cd does not change the session cwd.\n\n\
         In the metadata below, cwd and relative scope targets are Workdir-root-relative (`.` or an empty path denotes the root); explicit absolute provider scope targets stay absolute. \
         A null scope means the provider's full available scope, not unrestricted host access; an empty scope grants no paths. \
         Restrictions retain additional provider or ancestor ceilings, including deny rules. \
         Capabilities and command availability are session ceilings, not new grants. \
         Resolved targets remain subject to scope, symlink and command authorization. \
         SubWorker scope declarations remain Workdir-root-relative; other Scope declarations and Profile/Memory roots retain their own meaning and are not rebased by session cwd.\n\n\
         ```json\n{json}\n```"
    ))
}

/// Build the final system prompt by appending the fixed trailing
/// section to `body`. The Rust side owns the layout (blank-line
/// separators, trailing-whitespace trim); each section's header + body
/// comes from the prompt catalog (`WorkerPrompt::WorkingBoundariesSection`
/// / `WorkerPrompt::AgentsMdSection`) so that wording can be overridden
/// per-pack without touching this function.
fn append_trailing_section(
    body: &str,
    ctx: &SystemPromptContext<'_>,
    prompts: &PromptCatalog,
    scope: &Scope,
    agents_md: Option<&str>,
    resident_summary: Option<&str>,
) -> Result<String, SystemPromptError> {
    let mut out = String::with_capacity(body.len() + 256);
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out.push('\n');

    let notifications = prompts.render_name("common.notifications", Value::UNDEFINED)?;
    out.push_str(notifications.trim_end());
    out.push_str("\n\n");
    if ctx.workdir_sessions.is_empty() {
        let boundaries = prompts.working_boundaries_section(&scope.summary())?;
        out.push_str(boundaries.trim_end_matches(&['\n', ' '][..]));
        out.push('\n');
    }
    out.push_str(&workdir_environment_section(ctx.workdir_sessions)?);
    out.push('\n');
    if let Some(agents) = agents_md {
        out.push('\n');
        let section = prompts.agents_md_section(agents)?;
        out.push_str(section.trim_end_matches(&['\n', ' '][..]));
        out.push('\n');
    }
    if let Some(summary) = resident_summary {
        let summary = summary.trim_matches(&['\n', '\r'][..]);
        if !summary.trim().is_empty() {
            out.push('\n');
            let section = prompts.resident_memory_summary_section(summary)?;
            out.push_str(section.trim_end_matches(&['\n', ' '][..]));
            out.push('\n');
        }
    }
    for instruction in dedupe_instruction_contributions(ctx.feature_instructions.iter().cloned()) {
        out.push('\n');
        let prompt_ref = exact_prompt_name(&instruction.prompt_ref).ok_or_else(|| {
            SystemPromptError::Render(format!(
                "feature instruction must be an exact catalog-root dotted Prompt name: {}",
                instruction.prompt_ref
            ))
        })?;
        let section = prompts
            .render_name(&prompt_ref, ctx.to_minijinja_value())
            .map_err(|error| SystemPromptError::Render(error.to_string()))?;
        let section = section.trim_end_matches(&['\n', ' '][..]);
        if !section.trim().is_empty() {
            out.push_str(section);
            out.push('\n');
        }
    }
    // Canonicalise the tail so the emitted prompt has a single form
    // regardless of how individual templates chose to end.
    while out.ends_with('\n') || out.ends_with(' ') {
        out.pop();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::FeatureRegistryBuilder;
    use crate::feature::builtin::merge_request::MergeRequestFeature;
    use crate::feature::builtin::ticket::{TicketFeature, TicketFeatureAccess};
    use crate::hook::HookRegistryBuilder;
    use crate::worker::{
        WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceResponse,
    };
    use manifest::MergeRequestFeatureConfig;

    #[derive(Debug)]
    struct MetadataOnlyHost;
    impl WorkspaceClient for MetadataOnlyHost {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace")
        }
        fn kind(&self) -> &str {
            "metadata-fixture"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn authorize_feature_activation(&self, feature: &str) -> Result<(), WorkspaceClientError> {
            assert_eq!(feature, "ticket");
            Ok(())
        }
        fn execute(&self, _: WorkspaceRequest) -> Result<WorkspaceResponse, WorkspaceClientError> {
            panic!("metadata-only prompt fixture must not perform protected operations")
        }
    }

    fn ticket_workflow_tools(mr_config: MergeRequestFeatureConfig) -> Vec<agen::tool::ToolMeta> {
        ticket_tools_with_access(
            mr_config,
            TicketFeatureAccess {
                thread: true,
                workflow: true,
                ..Default::default()
            },
        )
    }

    fn ticket_tools_with_access(
        mr_config: MergeRequestFeatureConfig,
        access: TicketFeatureAccess,
    ) -> Vec<agen::tool::ToolMeta> {
        let client = Arc::new(MetadataOnlyHost);
        let mut tools = Vec::new();
        let mut hooks = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(TicketFeature::new(client.clone(), access))
            .with_module(MergeRequestFeature::new(client, mr_config))
            .install_into_pending(&mut tools, &mut hooks);
        assert!(!report.has_errors(), "{}", report.error_message());
        tools.into_iter().map(|definition| definition().0).collect()
    }

    #[test]
    fn ticket_case_update_schema_does_not_require_mr_approval() {
        let tools = ticket_workflow_tools(MergeRequestFeatureConfig::default());
        assert!(!tools.iter().any(|meta| meta.name == "ReviewMergeRequest"));
        let complete = tools
            .iter()
            .find(|meta| meta.name == "TicketCaseUpdate")
            .unwrap();
        assert!(
            complete.input_schema["properties"]
                .get("approval_event_id")
                .is_none()
        );
    }

    #[test]
    fn mr_completion_registration_and_schema_follow_feature_config() {
        let without_mr_tools = ticket_workflow_tools(MergeRequestFeatureConfig::default());
        assert!(
            !without_mr_tools
                .iter()
                .any(|meta| meta.name == "CompleteMergeRequest")
        );
        let tools = ticket_workflow_tools(MergeRequestFeatureConfig {
            show: true,
            readiness_check: true,
            complete: true,
            ..Default::default()
        });
        assert!(!tools.iter().any(|meta| meta.name == "ReviewMergeRequest"));
        let complete = tools
            .iter()
            .find(|meta| meta.name == "CompleteMergeRequest")
            .unwrap();
        assert!(
            complete.input_schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field == "approval_event_id")
        );
    }

    #[test]
    fn ticket_workflow_publishes_tracking_without_retired_operations() {
        let tools = ticket_workflow_tools(MergeRequestFeatureConfig::default());
        assert!(
            tools
                .iter()
                .any(|meta| meta.name == "TicketAssignmentTrackingUpdate")
        );
        assert!(!tools.iter().any(|meta| matches!(
            meta.name.as_str(),
            "TicketMarkReady"
                | "TicketQueue"
                | "TicketIntakeReady"
                | "CompleteTicket"
                | "TicketWorkflowState"
                | "TicketClose"
        )));
    }

    #[test]
    fn ticket_input_schemas_accept_repo_free_work_and_reject_actor_claims() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = manifest::ProfileResolver::new()
            .with_workspace_base(dir.path())
            .resolve(
                &manifest::ProfileSelector::parse_cli("builtin:orchestrator"),
                manifest::ProfileResolveOptions::with_worker_name("orchestrator-fixture"),
            )
            .unwrap();
        let config = resolved.manifest.feature.ticket;
        // Omit MR feature for the no-repository/no-MR work fixture; profile selection
        // makes it available, not mandatory to exercise for every Ticket.
        let tools = ticket_tools_with_access(
            MergeRequestFeatureConfig::default(),
            TicketFeatureAccess {
                authoring: config.authoring,
                thread: config.thread,
                intake: config.intake,
                workflow: config.workflow,
            },
        );
        assert!(!tools.iter().any(|meta| matches!(
            meta.name.as_str(),
            "OpenMergeRequest" | "ReviewMergeRequest" | "CompleteMergeRequest"
        )));
        for (name, input) in [
            (
                "TicketOrchestratorAssign",
                serde_json::json!({"ticket":"T-743","principal":{"kind":"subject","subject_id":"subject-a"},"expected_assignment_id":null,"operation_id":"delegate-research"}),
            ),
            (
                "TicketAssignmentTrackingUpdate",
                serde_json::json!({"ticket":"T-743", "assignment_id":"delegation", "expected_last_event_id":"assigned", "operation_key":"return-options", "tracking":{"status":"done", "result":"Options returned", "next_action":"Human decision"}}),
            ),
            (
                "TicketAssignmentTrackingUpdate",
                serde_json::json!({"ticket":"T-743", "assignment_id":"delegation", "expected_last_event_id":"assigned", "operation_key":"wait-for-answer", "tracking":{"status":"waiting", "waiting_reason":"User requested confirmation first", "next_action":"Read the user's answer on this assignment"}}),
            ),
            (
                "TicketAssignmentAnswer",
                serde_json::json!({"ticket":"T-743","assignment_id":"delegation","expected_last_event_id":"waiting","operation_key":"answer-on-same-row","body":"Option A approved; continue within scope"}),
            ),
            (
                "TicketCaseUpdate",
                serde_json::json!({"ticket":"T-743", "expected_content_digest":"observed-digest", "expected_state":"open", "expected_closure_operation_key":null, "operation_key":"fulfilled-close", "action":{"action":"close", "reason":"completed"}, "explanation":"The whole bounded request is fulfilled and closure was requested"}),
            ),
        ] {
            let meta = tools.iter().find(|meta| meta.name == name).unwrap();
            let validator = jsonschema::validator_for(&meta.input_schema).unwrap();
            assert!(validator.is_valid(&input), "{name} rejected {input}");
            let required = meta.input_schema["required"].as_array().unwrap();
            assert!(!required.iter().any(|field| matches!(
                field.as_str(),
                Some(
                    "repository_key"
                        | "merge_request_id"
                        | "approval_event_id"
                        | "workdir_attachments"
                )
            )));
            let mut forged = input;
            forged["actor"] = serde_json::json!("workspace-orchestrator");
            assert!(
                !validator.is_valid(&forged),
                "model-supplied authority accepted"
            );
        }
    }

    #[test]
    fn sub_worker_capabilities_follow_the_registered_canonical_control_tools() {
        let names = [
            "SubWorkerSpawn",
            "WorkerList",
            "WorkerSendInput",
            "WorkerStop",
        ]
        .map(str::to_string);
        let capabilities = ToolCapabilities::from_tool_names(&names);
        assert!(capabilities.sub_worker_management());
        assert!(capabilities.sub_worker_list);
        assert!(capabilities.sub_worker_send);
        assert!(capabilities.sub_worker_stop);

        let stale_aliases = [
            "SubWorkerSpawn",
            "SubWorkerList",
            "SubWorkerSend",
            "SubWorkerStop",
        ]
        .map(str::to_string);
        let capabilities = ToolCapabilities::from_tool_names(&stale_aliases);
        assert!(capabilities.sub_worker_spawn);
        assert!(!capabilities.sub_worker_list);
        assert!(!capabilities.sub_worker_send);
        assert!(!capabilities.sub_worker_stop);
    }

    #[test]
    fn rejects_legacy_prefix_relative_and_missing_names() {
        for reference in ["legacy/custom", "custom.md", "../custom", "missing"] {
            assert!(
                SystemPromptTemplate::parse(reference, PromptCatalogSource::builtins_only())
                    .is_err()
            );
        }
    }
}
