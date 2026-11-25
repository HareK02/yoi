//! Subject-scoped resident context and delegated Memory consolidation tools.

pub(crate) mod permission_declaration;

use crate::subjektiv::{SubjektivHostConnection, SubjektivHostError};
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::time::Duration;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

use crate::feature::{
    FeatureDescriptor, FeatureHookPoint, FeatureInstallContext, FeatureInstallError, FeatureModule,
    HookDeclaration, ToolContribution, ToolDeclaration,
};
use crate::hook::{
    BeforeSessionRewrite, BeforeSessionRewriteAction, BeforeSessionRewriteContext, Hook, HookError,
    HookErrorCategory, HookExecutionPolicy, HookPreRequestAction, PreLlmRequest, PreRequestContext,
    WorkerRestored,
};
use crate::prompt::catalog::{PromptCatalog, WorkerPrompt};
use crate::worker::{
    ResidentContextRefresh, SystemPromptContribution, SystemPromptContributionSource,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubjectBehaviorInput {
    pub behavior_md: String,
    pub prompt_provenance: Option<session_store::PromptRenderProvenance>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ResidentSummaryInput {
    prompt_provenance: Option<session_store::PromptRenderProvenance>,
}

fn resident_input(
    body: String,
    schema: &str,
    payload: &impl serde::Serialize,
    kind: &str,
) -> Result<session_store::LoggedSystemHistoryEntry, session_store::InputMetadataError> {
    use session_store::*;
    let input = SystemInputEnvelope::new(
        body.clone(),
        HistoryInputMetadata::new(
            HistoryInputUsePolicy {
                recording_material: InputEvidenceUse::Exclude,
                memory_evidence: InputEvidenceUse::Exclude,
                observation: InputObservationUse::Hidden,
                compaction: InputCompactionUse::Retain,
            },
            Some(FeatureInputPayload::encode(schema, 1, payload)?),
            Some(PublicSystemItem {
                kind: kind.into(),
                body,
                presentation: PublicSystemPresentation::Context,
            }),
        )?,
    )?;
    Ok(LoggedSystemHistoryEntry::from_input(
        LoggedSessionHistoryOrigin::Provided {
            producer: "subjektiv.resident-context".into(),
        },
        None,
        input,
    ))
}

pub(crate) fn subject_behavior_input(
    body: String,
    behavior_md: String,
    prompt_provenance: Option<session_store::PromptRenderProvenance>,
) -> Result<session_store::LoggedSystemHistoryEntry, session_store::InputMetadataError> {
    resident_input(
        body,
        "subject:behavior",
        &SubjectBehaviorInput {
            behavior_md,
            prompt_provenance,
        },
        "subject_behavior_refresh",
    )
}

pub(crate) fn resident_summary_input(
    body: String,
    prompt_provenance: Option<session_store::PromptRenderProvenance>,
) -> Result<session_store::LoggedSystemHistoryEntry, session_store::InputMetadataError> {
    resident_input(
        body,
        "memory:resident-summary",
        &ResidentSummaryInput { prompt_provenance },
        "resident_summary_refresh",
    )
}

#[cfg(test)]
const RESIDENT_SUMMARY_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn subject_memory_operation_name(
    operation: &server_api::SubjektivMemoryBackendOperation,
) -> &'static str {
    use server_api::SubjektivMemoryBackendOperation as Operation;
    match operation {
        Operation::ResidentSummary(_) | Operation::ResidentContext(_) => "memory.resident.read",
        Operation::Query(_) => "memory.query",
        Operation::Read(_) => "memory.read",
        Operation::ListChanges(_) => "memory.changes.list",
        Operation::ValidateProposal(_) => "memory.proposal.validate",
        Operation::ReceiptStatus(_) => "memory.receipt.read",
        Operation::StageExplicit(_) => "memory.staging.write",
        Operation::ListCandidates(_) => "memory.staging.list",
        Operation::ReadCandidate(_) => "memory.staging.read",
        Operation::DecideCandidate(_) => "memory.candidate.decide",
        Operation::PrepareSurface(_) => "memory.surface.prepare",
        Operation::PublishSurface(_) => "memory.surface.publish",
        Operation::FailSurface(_) => "memory.surface.fail",
    }
}

fn subjektiv_consolidation_tools(host: SubjektivHostConnection) -> Vec<ToolDefinition> {
    use SubjectConsolidationOperation as Operation;
    [
        (
            "SubjektivMemoryQuery",
            "Search current Memory changes for the delegated subject.",
            schema_for::<server_api::SubjektivMemoryQueryRequest>(),
            Operation::Query,
        ),
        (
            "SubjektivMemoryRead",
            "Read one current or historical Memory change for the delegated subject.",
            schema_for::<server_api::SubjektivMemoryReadRequest>(),
            Operation::Read,
        ),
        (
            "SubjektivMemoryListChanges",
            "List immutable changes of one Memory for the delegated subject.",
            schema_for::<server_api::SubjektivMemoryListChangesRequest>(),
            Operation::ListChanges,
        ),
        (
            "MemoryStagingList",
            "List pending staging candidates for the delegated subject.",
            schema_for::<server_api::SubjektivMemoryCandidateListRequest>(),
            Operation::ListCandidates,
        ),
        (
            "MemoryStagingRead",
            "Read one pending candidate, including typed change proposal metadata and provenance.",
            schema_for::<server_api::SubjektivMemoryCandidateReadRequest>(),
            Operation::ReadCandidate,
        ),
        (
            "MemoryApplyCandidate",
            "Atomically apply or reject one subject candidate. A request_id is idempotent; application revalidates exact change proposal metadata and returns concrete affected change refs.",
            schema_for::<server_api::SubjektivMemoryCandidateDecisionRequest>(),
            Operation::DecideCandidate,
        ),
    ]
    .into_iter()
    .map(|(name, description, schema, operation)| {
        let host = host.clone();
        Arc::new(move || {
            (
                ToolMeta::new(name)
                    .description(description)
                    .input_schema(schema.clone()),
                Arc::new(SubjectConsolidationTool {
                    host: host.clone(),
                    operation,
                }) as Arc<dyn Tool>,
            )
        }) as ToolDefinition
    })
    .collect()
}

#[derive(Clone, Copy)]
enum SubjectConsolidationOperation {
    Query,
    Read,
    ListChanges,
    ListCandidates,
    ReadCandidate,
    DecideCandidate,
}

struct SubjectConsolidationTool {
    host: SubjektivHostConnection,
    operation: SubjectConsolidationOperation,
}

#[async_trait]
impl Tool for SubjectConsolidationTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        use SubjectConsolidationOperation as Operation;
        let operation = match self.operation {
            Operation::Query => {
                server_api::SubjektivMemoryBackendOperation::Query(parse_input(input_json)?)
            }
            Operation::Read => {
                server_api::SubjektivMemoryBackendOperation::Read(parse_input(input_json)?)
            }
            Operation::ListChanges => {
                server_api::SubjektivMemoryBackendOperation::ListChanges(parse_input(input_json)?)
            }
            Operation::ListCandidates => {
                server_api::SubjektivMemoryBackendOperation::ListCandidates(parse_input(
                    input_json,
                )?)
            }
            Operation::ReadCandidate => {
                server_api::SubjektivMemoryBackendOperation::ReadCandidate(parse_input(input_json)?)
            }
            Operation::DecideCandidate => {
                server_api::SubjektivMemoryBackendOperation::DecideCandidate(parse_input(
                    input_json,
                )?)
            }
        };
        self.host
            .authorize_operation(subject_memory_operation_name(&operation))?;
        let response = match self.host.memory(operation) {
            Ok(response) => response,
            Err(SubjektivHostError::Conflict { code, message }) => {
                return Ok(ToolOutput {
                    summary: format!("Candidate decision requires reread: {code}."),
                    content: Some(serde_json::json!({"status":"error", "error":{"code":code,"message":message}}).to_string()),
                    attachments: Vec::new(),
                });
            }
            Err(error) => return Err(error.into()),
        };
        let summary = match &response {
            server_api::SubjektivMemoryBackendResponse::Query(value) => {
                format!("Found {} subject Memory item(s).", value.items.len())
            }
            server_api::SubjektivMemoryBackendResponse::Read(value) => {
                format!(
                    "Read Memory {} change {}.",
                    value.memory_id, value.change_id
                )
            }
            server_api::SubjektivMemoryBackendResponse::ListChanges(value) => {
                format!("Listed {} change(s).", value.items.len())
            }
            server_api::SubjektivMemoryBackendResponse::Candidates(value) => {
                format!("Listed {} pending candidate(s).", value.items.len())
            }
            server_api::SubjektivMemoryBackendResponse::Candidate(value) => {
                format!("Read pending candidate {}.", value.candidate_id)
            }
            server_api::SubjektivMemoryBackendResponse::CandidateDecided(value) => {
                format!("Candidate {} decision committed.", value.candidate_id)
            }
            _ => {
                return Err(ToolError::ExecutionFailed(
                    "unexpected subjektiv consolidation response".into(),
                ));
            }
        };
        Ok(ToolOutput {
            summary,
            content: Some(
                serde_json::to_string_pretty(&response)
                    .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?,
            ),
            attachments: Vec::new(),
        })
    }
}

fn parse_input<T: DeserializeOwned>(input: &str) -> Result<T, ToolError> {
    serde_json::from_str(input).map_err(|error| ToolError::InvalidArgument(error.to_string()))
}

fn schema_for<T: JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("memory tool schema should serialize")
}

#[derive(Default)]
struct SubjectBehaviorRefreshState {
    applied: Option<String>,
}

struct WorkspaceSubjektivResidentSummarySource {
    host: SubjektivHostConnection,
    prompts: Arc<ArcSwap<PromptCatalog>>,
    behavior_state: Arc<Mutex<SubjectBehaviorRefreshState>>,
}

impl WorkspaceSubjektivResidentSummarySource {
    fn confirm_resident_context_behavior(&self, behavior_md: &str) {
        self.behavior_state
            .lock()
            .expect("subject behavior state poisoned")
            .applied = Some(behavior_md.to_owned());
    }

    async fn fetch(
        &self,
    ) -> Result<server_api::SubjektivResidentContextOutput, SubjektivHostError> {
        self.host.authorize_operation("memory.resident.read")?;
        match self.host.memory(
            server_api::SubjektivMemoryBackendOperation::ResidentContext(Default::default()),
        )? {
            server_api::SubjektivMemoryBackendResponse::ResidentContext(output) => Ok(output),
            other => Err(SubjektivHostError::Unavailable(format!(
                "unexpected subject resident context response: {other:?}"
            ))),
        }
    }

    fn render(
        &self,
        output: &server_api::SubjektivResidentContextOutput,
    ) -> Result<(String, session_store::PromptRenderProvenance), String> {
        use memory::subject::MemoryResidentSummaryAvailability as Availability;
        let availability = match output.memory_surface.availability {
            Availability::Ready => "ready",
            Availability::Ungenerated => "ungenerated",
            Availability::Stale => "stale",
            Availability::Failed => "failed",
            Availability::Unavailable => "unavailable",
        };
        let prompts = self.prompts.load_full();
        let body = prompts
            .subjektiv_resident_context(
                &output.behavior_md,
                availability,
                output.memory_surface.content.as_deref(),
            )
            .map_err(|error| error.to_string())?;
        let projection = prompts.projection();
        Ok((
            body,
            session_store::PromptRenderProvenance {
                workspace_id: self.host.host.workspace_id(),
                source_digest: projection.source_digest.clone(),
                projection_digest: projection.catalog_digest.clone(),
                logical_name: "internal.subjektiv_resident_context".into(),
            },
        ))
    }
}

/// Detect behavior changes against the state reflected in history, not the fetched state.
///
/// Fetch/render never advances applied state. Initial preparation captures a
/// source-local completion, invoked only after its prompt and active pointer commit.
/// Later refreshes are acknowledged by the interceptor after durable append.
/// Invalidate applied before rewrite so identical bodies can be represented again.
#[async_trait]
impl SystemPromptContributionSource for WorkspaceSubjektivResidentSummarySource {
    fn protected_input_kind(&self) -> Option<&'static str> {
        Some("subject-resident")
    }

    async fn authorize_supply(&self) -> Result<(), String> {
        self.host
            .authorize_operation("memory.resident.supply")
            .map_err(|error| error.to_string())
    }

    async fn load(&self) -> SystemPromptContribution {
        self.load_initial().await.0
    }

    async fn load_initial(
        &self,
    ) -> (
        SystemPromptContribution,
        Option<crate::worker::PromptContributionCompletion>,
    ) {
        match self.load_initial_checked().await {
            Ok(contribution) => contribution,
            Err(error) => {
                tracing::error!(%error, "required Subject resident context not started; no prompt supplied");
                (SystemPromptContribution::Unavailable, None)
            }
        }
    }

    async fn load_initial_checked(
        &self,
    ) -> Result<
        (
            SystemPromptContribution,
            Option<crate::worker::PromptContributionCompletion>,
        ),
        String,
    > {
        self.authorize_supply().await?;
        let output = self.fetch().await.map_err(|error| error.to_string())?;
        let (rendered, _) = self.render(&output)?;
        self.authorize_supply().await?;
        let state = self.behavior_state.clone();
        let behavior = output.behavior_md;
        Ok((
            SystemPromptContribution::Ready(rendered),
            Some(Box::new(move || {
                state
                    .lock()
                    .expect("subject behavior state poisoned")
                    .applied = Some(behavior);
            })),
        ))
    }

    async fn load_changed_resident_context(
        &self,
    ) -> Result<Option<ResidentContextRefresh>, String> {
        self.authorize_supply().await?;
        let output = self.fetch().await.map_err(|error| error.to_string())?;
        let changed = self
            .behavior_state
            .lock()
            .expect("subject behavior state poisoned")
            .applied
            .as_ref()
            .is_none_or(|body| body != &output.behavior_md);
        if !changed {
            return Ok(None);
        }
        let (rendered, prompt_provenance) = self.render(&output)?;
        self.authorize_supply().await?;
        Ok(Some(ResidentContextRefresh {
            body: rendered,
            behavior_md: output.behavior_md,
            prompt_provenance: Some(prompt_provenance),
        }))
    }

    fn system_item_committed(
        &self,
        _item: &session_store::SystemItem,
        metadata: &crate::session_history::SessionHistoryMetadata,
    ) {
        if let Some(input) = metadata
            .input_metadata
            .as_ref()
            .and_then(|input| input.feature_payload())
            .and_then(|payload| {
                payload
                    .decode::<SubjectBehaviorInput>("subject:behavior", 1)
                    .ok()
            })
        {
            self.confirm_resident_context_behavior(&input.behavior_md);
        }
    }

    fn invalidate_resident_context_representation(&self) {
        self.behavior_state
            .lock()
            .expect("subject behavior state poisoned")
            .applied = None;
    }
}

const SUBJEKTIV_RESTORE_REFRESH_HOOK: &str = "refresh-resident-surface-after-restore";
const SUBJEKTIV_BEHAVIOR_REFRESH_HOOK: &str = "refresh-subject-behavior-before-request";
const SUBJEKTIV_BEHAVIOR_REWRITE_HOOK: &str = "invalidate-subject-behavior-before-rewrite";

/// Subjektiv-owned restore-boundary refresh.
///
/// Initial prompt contributions are a shared Host facility; only an execution-enabled, subject-connected
/// Feature requests restore refresh. Source presence alone must not make generic Worker core infer it.
/// Saved prompt/history is unchanged; one system item representing current resident context is appended.
/// A generated surface's ready-empty state differs from ungenerated, stale, or failed; those states
/// never mean empty Memory. Pre-request hooks handle later behavior updates; the restore boundary
/// itself is not repeated.
#[derive(Clone)]
pub(crate) struct SubjektivResidentRestoreRefreshFeature {
    source: Arc<dyn SystemPromptContributionSource>,
    prompts: Arc<ArcSwap<PromptCatalog>>,
    workspace_id: Option<String>,
}

impl SubjektivResidentRestoreRefreshFeature {
    #[cfg(test)]
    pub(crate) fn for_host(
        lifecycle_enabled: bool,
        source: Arc<dyn SystemPromptContributionSource>,
        prompts: Arc<ArcSwap<PromptCatalog>>,
        workspace_id: impl Into<String>,
    ) -> Option<Self> {
        lifecycle_enabled.then(|| Self::new(source, prompts, workspace_id))
    }

    pub(crate) fn for_connection(
        lifecycle_enabled: bool,
        source: Arc<dyn SystemPromptContributionSource>,
        prompts: Arc<ArcSwap<PromptCatalog>>,
        workspace_id: Option<String>,
    ) -> Option<Self> {
        lifecycle_enabled.then_some(Self {
            source,
            prompts,
            workspace_id,
        })
    }

    #[cfg(test)]
    pub(crate) fn new(
        source: Arc<dyn SystemPromptContributionSource>,
        prompts: Arc<ArcSwap<PromptCatalog>>,
        workspace_id: impl Into<String>,
    ) -> Self {
        Self {
            source,
            prompts,
            workspace_id: Some(workspace_id.into()),
        }
    }
}

impl FeatureModule for SubjektivResidentRestoreRefreshFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin(
            "subjektiv-resident-restore-refresh",
            "subjektiv Resident Restore Refresh",
        )
        .with_description(
            "Refreshes the current subject resident Memory surface at a Worker restore boundary.",
        )
        .with_hook(HookDeclaration::new(
            SUBJEKTIV_RESTORE_REFRESH_HOOK,
            FeatureHookPoint::WorkerRestored,
        ))
        .with_hook(HookDeclaration::new(
            SUBJEKTIV_BEHAVIOR_REFRESH_HOOK,
            FeatureHookPoint::PreLlmRequest,
        ))
        .with_hook(HookDeclaration::new(
            SUBJEKTIV_BEHAVIOR_REWRITE_HOOK,
            FeatureHookPoint::BeforeSessionRewrite,
        ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.hooks().add_worker_restored(
            SUBJEKTIV_RESTORE_REFRESH_HOOK,
            HookExecutionPolicy::fail_closed(),
            SubjektivResidentRestoreRefreshHook {
                source: Arc::clone(&self.source),
                prompts: Arc::clone(&self.prompts),
                workspace_id: self.workspace_id.clone(),
            },
        )?;
        context.hooks().add_pre_llm_request(
            SUBJEKTIV_BEHAVIOR_REFRESH_HOOK,
            HookExecutionPolicy::fail_closed(),
            SubjektivBehaviorRefreshHook {
                source: Arc::clone(&self.source),
            },
        )?;
        context.hooks().add_before_session_rewrite(
            SUBJEKTIV_BEHAVIOR_REWRITE_HOOK,
            HookExecutionPolicy::fail_closed(),
            SubjektivBehaviorRewriteHook {
                source: Arc::clone(&self.source),
            },
        )
    }
}

struct SubjektivBehaviorRewriteHook {
    source: Arc<dyn SystemPromptContributionSource>,
}

#[async_trait]
impl Hook<BeforeSessionRewrite> for SubjektivBehaviorRewriteHook {
    async fn call(
        &self,
        _context: &BeforeSessionRewriteContext,
    ) -> Result<BeforeSessionRewriteAction, HookError> {
        self.source.invalidate_resident_context_representation();
        Ok(BeforeSessionRewriteAction::Continue)
    }
}

struct SubjektivBehaviorRefreshHook {
    source: Arc<dyn SystemPromptContributionSource>,
}

#[async_trait]
impl Hook<PreLlmRequest> for SubjektivBehaviorRefreshHook {
    async fn call(&self, context: &PreRequestContext) -> Result<HookPreRequestAction, HookError> {
        let refresh = self
            .source
            .load_changed_resident_context()
            .await
            .map_err(|error| HookError::new(HookErrorCategory::Dependency, error))?;
        if let Some(refresh) = refresh {
            let system_items = context.system_items().ok_or_else(|| {
                HookError::new(
                    HookErrorCategory::Dependency,
                    "Subject behavior refresh requires durable session append authority",
                )
            })?;
            let input = subject_behavior_input(
                refresh.body,
                refresh.behavior_md,
                refresh.prompt_provenance,
            )
            .map_err(|error| HookError::new(HookErrorCategory::Dependency, error.to_string()))?;
            system_items.append_input(input);
        }
        Ok(HookPreRequestAction::Continue)
    }
}

struct SubjektivResidentRestoreRefreshHook {
    source: Arc<dyn SystemPromptContributionSource>,
    prompts: Arc<ArcSwap<PromptCatalog>>,
    workspace_id: Option<String>,
}

#[async_trait]
impl Hook<WorkerRestored> for SubjektivResidentRestoreRefreshHook {
    async fn call(&self, context: &crate::hook::WorkerRestoredContext) -> Result<(), HookError> {
        let (contribution, _) = self
            .source
            .load_initial_checked()
            .await
            .map_err(|error| HookError::new(HookErrorCategory::Dependency, error))?;
        self.source
            .authorize_supply()
            .await
            .map_err(|error| HookError::new(HookErrorCategory::Dependency, error))?;
        let (surface_ready, summary) = match &contribution {
            SystemPromptContribution::Ready(summary) => (true, Some(summary.as_str())),
            SystemPromptContribution::Unavailable => (false, None),
        };
        let prompts = self.prompts.load_full();
        let body = prompts
            .resident_memory_restore_section(surface_ready, summary)
            .map_err(|error| HookError::new(HookErrorCategory::Internal, error.to_string()))?;
        let projection = prompts.projection();
        let input = resident_summary_input(
            body,
            Some(session_store::PromptRenderProvenance {
                workspace_id: self.workspace_id.clone(),

                source_digest: projection.source_digest.clone(),
                projection_digest: projection.catalog_digest.clone(),
                logical_name: WorkerPrompt::ResidentMemoryRestoreSection.key().to_string(),
            }),
        )
        .map_err(|error| HookError::new(HookErrorCategory::Dependency, error.to_string()))?;
        context.system_items().append_input(input);
        Ok(())
    }
}

pub(crate) fn is_builtin_subjektiv_consolidation_profile(
    manifest: &manifest::WorkerManifest,
) -> bool {
    manifest.profile.as_ref().is_some_and(|snapshot| {
        matches!(
            &snapshot.source,
            manifest::ProfileSource::Registry {
                source: manifest::ProfileRegistrySource::Builtin,
                name,
                ..
            } if name == "subjektiv-memory-consolidation"
        )
    })
}

pub(crate) fn ordinary_subjektiv_features_enabled(manifest: &manifest::WorkerManifest) -> bool {
    manifest.feature.subjektiv.profile.enabled
        && !manifest.feature.subjektiv.profile.consolidation_tools
}

pub(crate) fn ordinary_subjektiv_resident_summary_source(
    manifest: &manifest::WorkerManifest,
    host: Option<SubjektivHostConnection>,
    prompts: Arc<ArcSwap<PromptCatalog>>,
) -> std::io::Result<Option<Arc<dyn SystemPromptContributionSource>>> {
    if !ordinary_subjektiv_features_enabled(manifest) || host.is_none() {
        return Ok(None);
    }
    manifest
        .feature
        .subjektiv
        .validate_execution()
        .map_err(std::io::Error::other)?;
    Ok(Some(Arc::new(WorkspaceSubjektivResidentSummarySource {
        host: host.expect("checked explicit connection"),
        prompts,
        behavior_state: Arc::new(Mutex::new(SubjectBehaviorRefreshState::default())),
    })))
}

pub(crate) struct SubjektivConsolidationFeatureInstallPlan {
    pub(crate) module: SubjektivConsolidationToolsFeature,
    pub(crate) system_prompt_override: String,
}

impl SubjektivConsolidationFeatureInstallPlan {
    pub(crate) fn prepare(
        manifest: &manifest::WorkerManifest,
        host: Option<SubjektivHostConnection>,
        prompts: Arc<crate::prompt::catalog::PromptCatalog>,
    ) -> std::io::Result<Option<Self>> {
        let config = &manifest.feature.subjektiv;
        if !config.profile.enabled || !config.profile.consolidation_tools || host.is_none() {
            return Ok(None);
        }
        config.validate_execution().map_err(std::io::Error::other)?;
        let host = host.expect("checked explicit connection");
        if config.profile.extraction.enabled || !host.host.consolidation_granted() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "subjektiv consolidation requires explicit Host grant without automatic extraction",
            ));
        }
        let system_prompt_override = prompts
            .subjektiv_memory_consolidation_system(&host.host.settings().language)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(Some(Self {
            module: SubjektivConsolidationToolsFeature::new(host)?,
            system_prompt_override,
        }))
    }
}

#[derive(Clone)]
pub struct SubjektivConsolidationToolsFeature {
    tools: Vec<ToolDefinition>,
}

impl SubjektivConsolidationToolsFeature {
    /// Reusable installation for a generic Job with an explicit attenuated Host.
    pub fn new(host: SubjektivHostConnection) -> std::io::Result<Self> {
        if !host.host.consolidation_granted() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Host did not grant consolidation",
            ));
        }
        Ok(Self {
            tools: subjektiv_consolidation_tools(host),
        })
    }
}

impl FeatureModule for SubjektivConsolidationToolsFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin(
            "subjektiv-memory-consolidation",
            "subjektiv Memory Consolidation",
        )
        .with_description("Dedicated subject Memory staging consolidation tools.");
        for tool in &self.tools {
            let (meta, _) = tool();
            descriptor = descriptor.with_tool(ToolDeclaration::new(meta.name, meta.description));
        }
        descriptor
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        for tool in &self.tools {
            let (meta, _) = tool();
            context
                .tools()
                .register(ToolContribution::new(meta.name, tool.clone()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};
    use agen::tool::ToolDefinition;
    use std::sync::Mutex;

    fn backend_resident_source(
        manifest: &manifest::WorkerManifest,
        client: Arc<dyn WorkspaceClient>,
        prompts: Arc<ArcSwap<PromptCatalog>>,
    ) -> std::io::Result<Option<Arc<dyn SystemPromptContributionSource>>> {
        let host = crate::subjektiv::BackendSubjektivHost::from_resolved_config(
            &manifest.feature.subjektiv,
            client,
        )?;
        let host = host
            .map(|host| {
                SubjektivHostConnection::new(
                    host,
                    crate::subjektiv::SubjektivHostContext {
                        worker_id: manifest.worker.name.clone(),
                        session_id: "session-1".into(),
                    },
                )
            })
            .transpose()?;
        ordinary_subjektiv_resident_summary_source(manifest, host, prompts)
    }

    fn test_prompts() -> Arc<ArcSwap<PromptCatalog>> {
        Arc::new(ArcSwap::from(PromptCatalog::builtins_only().unwrap()))
    }

    fn test_client() -> Arc<dyn WorkspaceClient> {
        Arc::new(crate::worker::TestWorkspaceHttpClient::new(
            "workspace",
            "http://backend",
        ))
    }

    #[derive(Debug)]
    struct SubjectResidentClient {
        availability: memory::subject::MemoryResidentSummaryAvailability,
        content: Option<String>,
        scope_allowed: std::sync::atomic::AtomicBool,
        behavior: Mutex<String>,
        paths: Mutex<Vec<String>>,
        timeouts: Mutex<Vec<Duration>>,
    }

    impl SubjectResidentClient {
        fn new(
            availability: memory::subject::MemoryResidentSummaryAvailability,
            content: Option<&str>,
        ) -> Self {
            Self {
                availability,
                content: content.map(str::to_string),
                scope_allowed: std::sync::atomic::AtomicBool::new(true),
                behavior: Mutex::new("Be deliberate.".to_string()),
                paths: Mutex::new(Vec::new()),
                timeouts: Mutex::new(Vec::new()),
            }
        }

        fn denied() -> Self {
            Self {
                availability: memory::subject::MemoryResidentSummaryAvailability::Ready,
                content: Some("must not be injected".into()),
                scope_allowed: std::sync::atomic::AtomicBool::new(false),
                behavior: Mutex::new("must not be injected".to_string()),
                paths: Mutex::new(Vec::new()),
                timeouts: Mutex::new(Vec::new()),
            }
        }
    }

    impl WorkspaceClient for SubjectResidentClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace")
        }

        fn kind(&self) -> &str {
            "subject-resident-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            if request.path.ends_with("/authorization/bound-check") {
                let request: server_api::authorization::BoundOperationAuthorizationRequest =
                    serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                assert!(matches!(
                    request.target,
                    server_api::authorization::BoundAuthorizationTarget::Subject {}
                ));
                return Ok(WorkspaceResponse {
                    content_type: None,
                    status: if self.scope_allowed.load(std::sync::atomic::Ordering::SeqCst) {
                        200
                    } else {
                        403
                    },
                    body: "{}".into(),
                });
            }
            self.paths.lock().unwrap().push(request.path.clone());
            assert!(request.path.ends_with("/subjektiv/memory"));
            let request: server_api::SubjektivMemoryBackendRequest =
                serde_json::from_str(request.body.as_deref().unwrap_or_default()).unwrap();
            assert!(matches!(
                request.operation,
                server_api::SubjektivMemoryBackendOperation::ResidentContext(_)
            ));
            if !self.scope_allowed.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(WorkspaceResponse {
                    content_type: None,
                    status: 403,
                    body: "subject scope unavailable".into(),
                });
            }
            let behavior_md = self.behavior.lock().unwrap().clone();
            Ok(WorkspaceResponse {
                content_type: None,
                status: 200,
                body: serde_json::to_string(
                    &server_api::SubjektivMemoryBackendResponse::ResidentContext(
                        server_api::SubjektivResidentContextOutput {
                            behavior_md,
                            memory_surface: memory::subject::MemoryResidentSummaryOutput {
                                availability: self.availability,
                                content: self.content.clone(),
                            },
                        },
                    ),
                )
                .unwrap(),
            })
        }

        fn execute_with_timeout(
            &self,
            request: WorkspaceRequest,
            timeout: Duration,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            if !request.path.contains("/authorization/") {
                self.timeouts.lock().unwrap().push(timeout);
            }
            self.execute(request)
        }
    }

    fn repository_error_client(status: u16, code: &str) -> Arc<dyn WorkspaceClient> {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = serde_json::json!({
            "error": "request rejected",
            "message": format!("{code}: retry after reread"),
            "diagnostics": [{
                "code": code,
                "severity": "error",
                "message": format!("{code}: retry after reread"),
            }],
        })
        .to_string();
        std::thread::spawn(move || {
            loop {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let size = stream.read(&mut request).unwrap();
                let is_authorization =
                    String::from_utf8_lossy(&request[..size]).contains("/authorization/");
                let (status, response_body) = if is_authorization {
                    (200, "{}")
                } else {
                    (status, body.as_str())
                };
                write!(stream, "HTTP/1.1 {status} response\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response_body}", response_body.len()).unwrap();
                if !is_authorization {
                    break;
                }
            }
        });
        Arc::new(crate::worker::TestWorkspaceHttpClient::new(
            "workspace",
            format!("http://{addr}"),
        ))
    }

    fn tool_names(definitions: Vec<ToolDefinition>) -> Vec<String> {
        let mut names = definitions
            .into_iter()
            .map(|tool| tool().0.name)
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn tool_meta(definitions: Vec<ToolDefinition>, name: &str) -> serde_json::Value {
        definitions
            .into_iter()
            .map(|tool| tool().0)
            .find(|meta| meta.name == name)
            .unwrap_or_else(|| panic!("missing tool meta for {name}"))
            .input_schema
    }

    #[test]
    fn dedicated_subjektiv_consolidator_uses_subject_settings_and_only_consolidation_tools() {
        let workspace = tempfile::tempdir().unwrap();
        let mut manifest = manifest::ProfileResolver::new()
            .with_workspace_base(workspace.path())
            .resolve(
                &manifest::ProfileSelector::source_named(
                    manifest::ProfileRegistrySource::Builtin,
                    "subjektiv-memory-consolidation",
                ),
                manifest::ProfileResolveOptions::with_worker_name("subject-consolidator"),
            )
            .unwrap()
            .manifest;
        manifest
            .feature
            .subjektiv
            .bind_workspace_settings(manifest::SubjektivMemorySettingsSnapshot {
                workspace_id: "workspace".into(),

                language: "English".into(),
            })
            .unwrap();

        assert!(!ordinary_subjektiv_features_enabled(&manifest));

        let plan = SubjektivConsolidationFeatureInstallPlan::prepare(
            &manifest,
            Some(crate::subjektiv::test_connection(test_client())),
            crate::prompt::catalog::PromptCatalog::builtins_only().unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            plan.module.descriptor().id.as_str(),
            "builtin:subjektiv-memory-consolidation"
        );
        let names = tool_names(plan.module.tools);
        assert!(names.contains(&"SubjektivMemoryQuery".to_string()));
        assert!(names.contains(&"MemoryApplyCandidate".to_string()));
        assert!(!names.contains(&"MemoryQuery".to_string()));
        assert!(!names.contains(&"SubjektivRemember".to_string()));
    }

    fn subject_manifest(workspace_id: &str) -> manifest::WorkerManifest {
        let mut manifest = manifest::WorkerManifest::from_toml(
            r#"
[worker]
name = "subject-worker"
scope = "main"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]

[[scope.allow]]
target = "/subject-resident-test"
permission = "write"
"#,
        )
        .unwrap();
        manifest.feature.subjektiv.profile.enabled = true;
        manifest
            .feature
            .subjektiv
            .bind_workspace_settings(manifest::SubjektivMemorySettingsSnapshot {
                workspace_id: workspace_id.to_string(),

                language: "English".to_string(),
            })
            .unwrap();
        manifest
    }

    #[tokio::test]
    async fn subject_resident_source_preserves_only_current_ready_surface() {
        let manifest = subject_manifest("workspace");
        let ready_client = Arc::new(SubjectResidentClient::new(
            memory::subject::MemoryResidentSummaryAvailability::Ready,
            Some("current subject surface"),
        ));
        let ready = backend_resident_source(&manifest, ready_client.clone(), test_prompts())
            .unwrap()
            .unwrap();
        let loaded = ready.load().await;
        let SystemPromptContribution::Ready(loaded) = loaded else {
            panic!("subject resident context must be present");
        };
        assert!(loaded.contains("Be deliberate."));
        assert!(loaded.contains("current subject surface"));
        assert_eq!(
            ready_client.paths.lock().unwrap().clone(),
            vec!["/api/w/workspace/subjektiv/memory".to_string()]
        );
        assert_eq!(
            ready_client.timeouts.lock().unwrap().as_slice(),
            &[RESIDENT_SUMMARY_REQUEST_TIMEOUT]
        );

        let ready_empty_client = Arc::new(SubjectResidentClient::new(
            memory::subject::MemoryResidentSummaryAvailability::Ready,
            None,
        ));
        let ready_empty = backend_resident_source(&manifest, ready_empty_client, test_prompts())
            .unwrap()
            .unwrap();
        let SystemPromptContribution::Ready(ready_empty) = ready_empty.load().await else {
            panic!("empty surface must retain Subject behavior");
        };
        assert!(ready_empty.contains("Be deliberate."));
        assert!(ready_empty.contains("intentionally empty"));

        for availability in [
            memory::subject::MemoryResidentSummaryAvailability::Ungenerated,
            memory::subject::MemoryResidentSummaryAvailability::Stale,
            memory::subject::MemoryResidentSummaryAvailability::Failed,
        ] {
            let source = backend_resident_source(
                &manifest,
                Arc::new(SubjectResidentClient::new(
                    availability,
                    Some("must not be injected"),
                )),
                test_prompts(),
            )
            .unwrap()
            .unwrap();
            let SystemPromptContribution::Ready(loaded) = source.load().await else {
                panic!("Subject behavior must survive unavailable surfaces");
            };
            assert!(loaded.contains("Be deliberate."));
            assert!(!loaded.contains("must not be injected"));
        }
    }

    #[tokio::test]
    async fn subject_resident_source_emits_only_changed_behavior() {
        let manifest = subject_manifest("workspace");
        let client = Arc::new(SubjectResidentClient::new(
            memory::subject::MemoryResidentSummaryAvailability::Ungenerated,
            None,
        ));
        let source = backend_resident_source(&manifest, client.clone(), test_prompts())
            .unwrap()
            .unwrap();

        let (initial, completion) = source.load_initial().await;
        assert!(matches!(initial, SystemPromptContribution::Ready(_)));
        assert!(
            source
                .load_changed_resident_context()
                .await
                .unwrap()
                .is_some()
        );
        completion.expect("successful initial contribution must provide completion")();
        assert!(
            source
                .load_changed_resident_context()
                .await
                .unwrap()
                .is_none()
        );

        *client.behavior.lock().unwrap() = "Ask when uncertain.".to_string();
        let refresh = source
            .load_changed_resident_context()
            .await
            .unwrap()
            .expect("changed behavior must produce one refresh");
        assert_eq!(refresh.behavior_md, "Ask when uncertain.");
        assert!(refresh.body.contains("Ask when uncertain."));
        let input = subject_behavior_input(
            refresh.body.clone(),
            refresh.behavior_md.clone(),
            refresh.prompt_provenance.clone(),
        )
        .unwrap();
        source.system_item_committed(&input.item, &input.metadata);
        assert!(
            source
                .load_changed_resident_context()
                .await
                .unwrap()
                .is_none()
        );

        *client.behavior.lock().unwrap() = String::new();
        let cleared = source
            .load_changed_resident_context()
            .await
            .unwrap()
            .expect("cleared behavior must produce one refresh");
        assert_eq!(cleared.behavior_md, "");
        assert!(cleared.body.contains("No user-managed behavior is set"));
        let input = subject_behavior_input(
            cleared.body.clone(),
            cleared.behavior_md.clone(),
            cleared.prompt_provenance.clone(),
        )
        .unwrap();
        source.system_item_committed(&input.item, &input.metadata);
        assert!(
            source
                .load_changed_resident_context()
                .await
                .unwrap()
                .is_none()
        );

        source.invalidate_resident_context_representation();
        let replay_after_rewrite = source
            .load_changed_resident_context()
            .await
            .unwrap()
            .expect("history rewrite must require a durable current representation");
        assert_eq!(replay_after_rewrite.behavior_md, "");
        assert!(
            replay_after_rewrite
                .body
                .contains("No user-managed behavior is set")
        );
        // Initial load and every live update/clear/replay probe retain the
        // target-side bounded resident request contract.
        assert_eq!(
            client.timeouts.lock().unwrap().as_slice(),
            &[RESIDENT_SUMMARY_REQUEST_TIMEOUT; 8]
        );
    }

    #[tokio::test]
    async fn resident_behavior_ack_tracks_exact_committed_content_not_latest_fetch() {
        let manifest = subject_manifest("workspace");
        let client = Arc::new(SubjectResidentClient::new(
            memory::subject::MemoryResidentSummaryAvailability::Ungenerated,
            None,
        ));
        let source = backend_resident_source(&manifest, client.clone(), test_prompts())
            .unwrap()
            .unwrap();
        source.load().await;
        *client.behavior.lock().unwrap() = "first behavior".into();
        let first = source
            .load_changed_resident_context()
            .await
            .unwrap()
            .unwrap();
        let retry = source
            .load_changed_resident_context()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retry, first); // Fetch alone never acknowledges model representation.
        *client.behavior.lock().unwrap() = "second behavior".into();
        let second = source
            .load_changed_resident_context()
            .await
            .unwrap()
            .unwrap();
        let input = subject_behavior_input(
            first.body.clone(),
            first.behavior_md.clone(),
            first.prompt_provenance.clone(),
        )
        .unwrap();
        source.system_item_committed(&input.item, &input.metadata);
        assert_eq!(
            source
                .load_changed_resident_context()
                .await
                .unwrap()
                .unwrap(),
            second
        );
        let input = subject_behavior_input(
            second.body.clone(),
            second.behavior_md.clone(),
            second.prompt_provenance.clone(),
        )
        .unwrap();
        source.system_item_committed(&input.item, &input.metadata);
        assert!(
            source
                .load_changed_resident_context()
                .await
                .unwrap()
                .is_none()
        );
        source.invalidate_resident_context_representation();
        assert_eq!(
            source
                .load_changed_resident_context()
                .await
                .unwrap()
                .unwrap(),
            second
        );
    }

    #[tokio::test]
    async fn subject_resident_source_fails_closed_without_trusted_subject_scope() {
        let manifest = subject_manifest("workspace");
        let client = Arc::new(SubjectResidentClient::denied());
        let source = backend_resident_source(&manifest, client.clone(), test_prompts())
            .unwrap()
            .unwrap();

        let error = match source.load_initial_checked().await {
            Err(error) => error,
            Ok(_) => panic!("required resident fetch must fail startup"),
        };
        assert!(error.contains("403"));
        assert_eq!(client.paths.lock().unwrap().clone(), Vec::<String>::new());
    }

    #[test]
    fn subject_resident_source_requires_trusted_matching_workspace_config() {
        let disabled = manifest::WorkerManifest::from_toml(
            r#"
[worker]
name = "ordinary-worker"
scope = "main"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]

[[scope.allow]]
target = "/subject-resident-test"
permission = "write"
"#,
        )
        .unwrap();
        assert!(
            backend_resident_source(&disabled, test_client(), test_prompts())
                .unwrap()
                .is_none()
        );

        let mut policy_only = disabled.clone();
        policy_only.feature.subjektiv.profile.enabled = true;
        let unattached_client = Arc::new(SubjectResidentClient::new(
            memory::subject::MemoryResidentSummaryAvailability::Ready,
            Some("policy-only surface must not load"),
        ));
        assert!(
            backend_resident_source(&policy_only, unattached_client.clone(), test_prompts())
                .unwrap()
                .is_none()
        );
        assert!(unattached_client.paths.lock().unwrap().is_empty());

        let foreign = subject_manifest("other-workspace");
        assert!(backend_resident_source(&foreign, test_client(), test_prompts()).is_err());
    }

    #[test]
    fn subject_restore_refresh_is_suppressed_for_internal_hosts() {
        let manifest = subject_manifest("workspace");
        let source = backend_resident_source(&manifest, test_client(), test_prompts())
            .unwrap()
            .unwrap();
        let prompts = Arc::new(ArcSwap::from(
            crate::prompt::catalog::PromptCatalog::builtins_only().unwrap(),
        ));

        assert!(
            SubjektivResidentRestoreRefreshFeature::for_host(false, source, prompts, "workspace",)
                .is_none()
        );
    }

    #[test]
    fn subject_restore_refresh_feature_declares_only_restore_lifecycle() {
        let manifest = subject_manifest("workspace");
        let source = backend_resident_source(&manifest, test_client(), test_prompts())
            .unwrap()
            .unwrap();
        let feature = SubjektivResidentRestoreRefreshFeature::new(
            source,
            Arc::new(ArcSwap::from(
                crate::prompt::catalog::PromptCatalog::builtins_only().unwrap(),
            )),
            "workspace",
        );
        let descriptor = feature.descriptor();

        assert!(descriptor.tools.is_empty());
        assert_eq!(descriptor.hooks.len(), 3);
        assert_eq!(descriptor.hooks[0].point, FeatureHookPoint::WorkerRestored);
        assert_eq!(descriptor.hooks[1].point, FeatureHookPoint::PreLlmRequest);
        assert_eq!(
            descriptor.hooks[2].point,
            FeatureHookPoint::BeforeSessionRewrite
        );
    }

    #[tokio::test]
    async fn subject_cached_and_changed_resident_supply_reject_revocation_without_new_reads() {
        let manifest = subject_manifest("workspace");
        let client = Arc::new(SubjectResidentClient::new(
            memory::subject::MemoryResidentSummaryAvailability::Ready,
            Some("private surface"),
        ));
        let source = backend_resident_source(&manifest, client.clone(), test_prompts())
            .unwrap()
            .unwrap();
        let (initial, completion) = source.load_initial_checked().await.unwrap();
        assert!(matches!(initial, SystemPromptContribution::Ready(_)));
        completion.unwrap()();
        assert_eq!(client.paths.lock().unwrap().len(), 1);
        client
            .scope_allowed
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(
            source
                .authorize_supply()
                .await
                .unwrap_err()
                .contains("denied")
        );
        assert!(source.load_changed_resident_context().await.is_err());
        assert_eq!(client.paths.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn subjektiv_candidate_conflicts_survive_workspace_http_transport() {
        let input = serde_json::json!({
            "request_id": "decision-request-1",
            "candidate_id": "candidate-1",
            "reason": "candidate is already covered",
            "decision": {
                "kind": "close",
                "action": "already_covered",
                "affected_memory": [],
            },
        })
        .to_string();
        for (status, code) in [
            (409, "candidate_decision_conflict"),
            (403, "subject_scope_mismatch"),
        ] {
            let tool = SubjectConsolidationTool {
                host: crate::subjektiv::test_connection(repository_error_client(status, code)),
                operation: SubjectConsolidationOperation::DecideCandidate,
            };
            let result = tool.execute(&input, ToolExecutionContext::direct()).await;
            if status == 403 {
                let error =
                    result.expect_err("provider permission denial is not a retryable conflict");
                assert!(error.to_string().contains("permission denied"));
                assert!(error.to_string().contains(code));
                continue;
            }
            let output = result.expect("typed conflicts are model-visible retry output");
            let content: serde_json::Value =
                serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
            assert_eq!(content["status"], "error");
            assert_eq!(content["error"]["code"], code);
            assert!(output.summary.contains("requires reread"));
        }
    }

    #[test]
    fn subjektiv_consolidation_has_atomic_decision_without_host_scope_fields() {
        let tools = subjektiv_consolidation_tools(crate::subjektiv::test_connection(test_client()));
        let names = tool_names(tools.clone());
        assert_eq!(
            names,
            vec![
                "MemoryApplyCandidate",
                "MemoryStagingList",
                "MemoryStagingRead",
                "SubjektivMemoryListChanges",
                "SubjektivMemoryQuery",
                "SubjektivMemoryRead",
            ]
        );
        let decision = tool_meta(tools, "MemoryApplyCandidate").to_string();
        assert!(decision.contains("request_id"));
        assert!(decision.contains("expected_change_id"));
        assert!(decision.contains("already_covered"));
        assert!(!decision.contains("subject_id"));
        assert!(!decision.contains("workspace_id"));
        assert!(!decision.contains("runtime_id"));
        assert!(!decision.contains("worker_id"));
        assert!(!decision.contains("source_candidate_ids"));
    }
}
