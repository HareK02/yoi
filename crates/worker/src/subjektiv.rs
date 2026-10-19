//! Explicit Host capability for subject-scoped Memory and committed Sessions.
//!
//! A manifest is policy, not a connection. Hosts bind Subject/storage authority
//! in their implementation; the Worker supplies its own Worker/Session context.
//! Neither Subject identity nor filesystem/Workspace authority is model input.

pub use crate::feature::builtin::memory::SubjektivConsolidationToolsFeature;
pub use crate::feature::builtin::memory_surface_lifecycle::SubjektivSurfaceLifecycleFeature;

pub use crate::feature::builtin::journal::{
    JournalJobFinalizer, JournalJobHost, JournalJobToolsFeature, JournalRecordingFeature,
};
pub use subjektiv::journal::memory_links::{MemoryJournalRequest, MemoryJournalResponse};
use subjektiv::journal::startup::JournalStartupSnapshot;
pub use subjektiv::journal::transport;
use transport::{JournalRecordingReason, JournalRecordingRequest, JournalRecordingResponse};

use std::sync::Arc;
use std::time::Duration;

use agen::tool::ToolError;
use memory::backend::{
    MemoryConsolidateStagingOperation, MemoryConsolidationOutput, MemoryStageCandidateOperation,
};
use server_api::*;

use crate::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod, WorkspaceServerOperation};

/// Host-issued execution context, never deserialized from model arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubjektivHostContext {
    pub worker_id: String,
    pub session_id: String,
}

/// Explicit local configuration. This is not a trusted Workspace snapshot and
/// is not persisted in the reusable Worker manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubjektivHostSettings {
    pub language: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SubjektivHostError {
    #[error("{message}")]
    Conflict { code: String, message: String },
    #[error("{0}")]
    InvalidArgument(String),
    #[error("{0}")]
    Unavailable(String),
}

impl From<SubjektivHostError> for ToolError {
    fn from(error: SubjektivHostError) -> Self {
        match error {
            SubjektivHostError::Conflict { code, message } => {
                Self::StructuredConflict { code, message }
            }
            SubjektivHostError::InvalidArgument(message) => Self::InvalidArgument(message),
            SubjektivHostError::Unavailable(message) => Self::ExecutionFailed(message),
        }
    }
}

/// Object-safe capability implemented by Backend and standalone Hosts.
/// Implementations must enforce their bound Subject, Worker/Session authority,
/// committed evidence and (for Jobs) immutable batch attenuation. Knowing an
/// identifier is not a grant. No method grants ambient filesystem access.
pub trait SubjektivHost: Send + Sync + 'static {
    fn settings(&self) -> SubjektivHostSettings;
    /// Startup reads are a body-only capability, independent of recording Jobs.
    fn journal_startup_granted(&self) -> bool {
        false
    }
    fn journal_startup_context(
        &self,
        _context: &SubjektivHostContext,
    ) -> Result<JournalStartupSnapshot, SubjektivHostError> {
        Err(SubjektivHostError::Unavailable(
            "Journal startup not granted".into(),
        ))
    }
    fn journal_compaction_granted(&self) -> bool {
        false
    }
    fn journal_compaction_context(
        &self,
        _context: &SubjektivHostContext,
    ) -> Result<JournalStartupSnapshot, SubjektivHostError> {
        Err(SubjektivHostError::Unavailable(
            "Journal compaction not granted".into(),
        ))
    }
    /// Recording admission is separate from both Memory and Journal Job operation grants.
    fn journal_recording_granted(&self) -> bool {
        false
    }
    fn request_journal_recording(
        &self,
        _context: &SubjektivHostContext,
        _reason: JournalRecordingReason,
    ) -> Result<JournalRecordingResponse, SubjektivHostError> {
        Err(SubjektivHostError::Unavailable(
            "Journal recording not granted".into(),
        ))
    }
    /// Additive provenance links and direct Journal reads, separate from recording/Memory mutation.
    /// Restricted Job Hosts inherit denial rather than the main Subject reader capability.
    fn memory_journal(
        &self,
        _context: &SubjektivHostContext,
        _request: MemoryJournalRequest,
    ) -> Result<MemoryJournalResponse, SubjektivHostError> {
        Err(SubjektivHostError::Unavailable(
            "Memory Journal references not granted".into(),
        ))
    }
    /// Separate explicit grant for decision/surface execution; Profile policy
    /// alone must never authorize consolidation tools.
    fn consolidation_granted(&self) -> bool {
        false
    }
    /// Optional real Backend identity, used only for prompt provenance.
    fn workspace_id(&self) -> Option<String> {
        None
    }
    fn memory(
        &self,
        context: &SubjektivHostContext,
        operation: SubjektivMemoryBackendOperation,
    ) -> Result<SubjektivMemoryBackendResponse, SubjektivHostError>;
    fn session(
        &self,
        context: &SubjektivHostContext,
        operation: SubjektivSessionBackendOperation,
    ) -> Result<SubjektivSessionBackendResponse, SubjektivHostError>;
    fn stage_candidate(
        &self,
        context: &SubjektivHostContext,
        operation: MemoryStageCandidateOperation,
    ) -> Result<SubjektivStageCandidateResponse, SubjektivHostError>;
    fn record_session(
        &self,
        context: &SubjektivHostContext,
        create_if_missing: bool,
    ) -> Result<SubjektivRecordSessionResponse, SubjektivHostError>;
    fn request_consolidation(
        &self,
        context: &SubjektivHostContext,
        operation: MemoryConsolidateStagingOperation,
    ) -> Result<MemoryConsolidationOutput, SubjektivHostError>;
}

/// Backend adapter. Existing authenticated Workspace operations and snapshot
/// validation remain the authority; no local Workspace is synthesized.
pub struct BackendSubjektivHost {
    client: Arc<dyn WorkspaceClient>,
    settings: SubjektivHostSettings,
    workspace_id: String,
    consolidation_granted: bool,
}

impl BackendSubjektivHost {
    pub fn from_resolved_config(
        config: &manifest::ResolvedSubjektivFeatureConfig,
        client: Arc<dyn WorkspaceClient>,
    ) -> std::io::Result<Option<Arc<dyn SubjektivHost>>> {
        config.validate_execution().map_err(std::io::Error::other)?;
        if !config.execution_enabled() {
            return Ok(None);
        }
        let settings = config
            .workspace_settings()
            .expect("execution requires trusted settings");
        if !client.is_available() || client.workspace_id() != Some(settings.workspace_id.as_str()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "subjektiv requires available Backend authority matching trusted Workspace settings",
            ));
        }
        Ok(Some(Arc::new(Self {
            client,
            workspace_id: settings.workspace_id,
            settings: SubjektivHostSettings {
                language: settings.language,
            },
            consolidation_granted: config.profile.consolidation_tools,
        })))
    }

    fn execute<T: serde::de::DeserializeOwned>(
        &self,
        operation: WorkspaceServerOperation,
    ) -> Result<T, SubjektivHostError> {
        let response = self
            .client
            .execute_server_operation(operation)
            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
        decode(response)
    }
}

fn decode<T: serde::de::DeserializeOwned>(
    response: crate::WorkspaceResponse,
) -> Result<T, SubjektivHostError> {
    if !response.is_success() {
        let parsed = serde_json::from_str::<RepositoryApiError>(&response.body).ok();
        let message = parsed
            .as_ref()
            .map(|error| error.message.clone())
            .unwrap_or(response.body);
        if let Some(code) = parsed.as_ref().and_then(|error| {
            error
                .diagnostics
                .iter()
                .find(|diagnostic| {
                    matches!(
                        diagnostic.code.as_str(),
                        "change_conflict"
                            | "stale_cursor"
                            | "candidate_decision_conflict"
                            | "subject_scope_mismatch"
                    )
                })
                .map(|diagnostic| diagnostic.code.clone())
        }) {
            return Err(SubjektivHostError::Conflict { code, message });
        }
        return Err(if response.status == 409 {
            SubjektivHostError::Conflict {
                code: "conflict".into(),
                message,
            }
        } else if matches!(response.status, 400 | 404 | 422) {
            SubjektivHostError::InvalidArgument(message)
        } else {
            SubjektivHostError::Unavailable(message)
        });
    }
    serde_json::from_str(&response.body).map_err(|error| {
        SubjektivHostError::Unavailable(format!("decode subjektiv response: {error}"))
    })
}

impl SubjektivHost for BackendSubjektivHost {
    fn memory_journal(
        &self,
        _context: &SubjektivHostContext,
        request: MemoryJournalRequest,
    ) -> Result<MemoryJournalResponse, SubjektivHostError> {
        decode(
            self.client
                .execute(WorkspaceRequest::json(
                    WorkspaceRequestMethod::Post,
                    format!("/api/w/{}/subjektiv/journal/memory", self.workspace_id),
                    serde_json::to_string(&request)
                        .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
                ))
                .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
        )
    }
    fn journal_startup_granted(&self) -> bool {
        true
    }
    fn journal_startup_context(
        &self,
        _context: &SubjektivHostContext,
    ) -> Result<JournalStartupSnapshot, SubjektivHostError> {
        decode(
            self.client
                .execute_with_timeout(
                    WorkspaceRequest::get(format!(
                        "/api/w/{}/subjektiv/journal/startup",
                        self.workspace_id
                    )),
                    Duration::from_secs(5),
                )
                .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
        )
    }
    fn journal_compaction_granted(&self) -> bool {
        true
    }
    fn journal_compaction_context(
        &self,
        context: &SubjektivHostContext,
    ) -> Result<JournalStartupSnapshot, SubjektivHostError> {
        decode(
            self.client
                .execute_with_timeout(
                    WorkspaceRequest::json(
                        WorkspaceRequestMethod::Post,
                        format!("/api/w/{}/subjektiv/journal/compaction", self.workspace_id),
                        serde_json::to_string(&transport::JournalCompactionRequest {
                            session_id: context.session_id.clone(),
                        })
                        .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
                    ),
                    Duration::from_secs(5),
                )
                .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
        )
    }
    fn journal_recording_granted(&self) -> bool {
        // This adapter exists only for explicitly enabled, authenticated Host connections.
        // The Backend still verifies the main Worker/Subject binding on every request.
        true
    }
    fn request_journal_recording(
        &self,
        _context: &SubjektivHostContext,
        reason: JournalRecordingReason,
    ) -> Result<JournalRecordingResponse, SubjektivHostError> {
        if !self.journal_recording_granted() {
            return Err(SubjektivHostError::Unavailable(
                "Journal recording not granted".into(),
            ));
        }
        let request = WorkspaceRequest::json(
            WorkspaceRequestMethod::Post,
            format!("/api/w/{}/subjektiv/journal/recording", self.workspace_id),
            serde_json::to_string(&JournalRecordingRequest { reason })
                .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
        );
        decode(
            self.client
                .execute(request)
                .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
        )
    }
    fn settings(&self) -> SubjektivHostSettings {
        SubjektivHostSettings {
            language: self.settings.language.clone(),
        }
    }
    fn workspace_id(&self) -> Option<String> {
        Some(self.workspace_id.clone())
    }
    fn consolidation_granted(&self) -> bool {
        self.consolidation_granted
    }
    fn memory(
        &self,
        _context: &SubjektivHostContext,
        operation: SubjektivMemoryBackendOperation,
    ) -> Result<SubjektivMemoryBackendResponse, SubjektivHostError> {
        // Preserve the bounded resident-context request timeout.
        if matches!(
            operation,
            SubjektivMemoryBackendOperation::ResidentContext(_)
        ) {
            let request = WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                format!("/api/w/{}/subjektiv/memory", self.workspace_id),
                serde_json::to_string(&SubjektivMemoryBackendRequest { operation })
                    .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
            );
            return decode(
                self.client
                    .execute_with_timeout(request, Duration::from_secs(5))
                    .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
            );
        }
        self.execute(WorkspaceServerOperation::SubjektivMemory(
            SubjektivMemoryBackendRequest { operation },
        ))
    }
    fn session(
        &self,
        _context: &SubjektivHostContext,
        operation: SubjektivSessionBackendOperation,
    ) -> Result<SubjektivSessionBackendResponse, SubjektivHostError> {
        let response = self
            .client
            .execute_server_operation(WorkspaceServerOperation::SubjektivSession(
                SubjektivSessionBackendRequest { operation },
            ))
            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
        // Session domain diagnostics are typed even on non-2xx responses.
        if let Ok(value) = serde_json::from_str(&response.body) {
            return Ok(value);
        }
        decode(response)
    }
    fn stage_candidate(
        &self,
        context: &SubjektivHostContext,
        operation: MemoryStageCandidateOperation,
    ) -> Result<SubjektivStageCandidateResponse, SubjektivHostError> {
        self.execute(WorkspaceServerOperation::SubjektivStageCandidate(
            SubjektivStageCandidateRequest {
                session_id: context.session_id.clone(),
                operation,
            },
        ))
    }
    fn record_session(
        &self,
        context: &SubjektivHostContext,
        create_if_missing: bool,
    ) -> Result<SubjektivRecordSessionResponse, SubjektivHostError> {
        self.execute(WorkspaceServerOperation::SubjektivRecordSession(
            SubjektivRecordSessionRequest {
                session_id: context.session_id.clone(),
                create_if_missing,
            },
        ))
    }
    fn request_consolidation(
        &self,
        _context: &SubjektivHostContext,
        operation: MemoryConsolidateStagingOperation,
    ) -> Result<MemoryConsolidationOutput, SubjektivHostError> {
        let response = self
            .client
            .execute(WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                format!("/api/w/{}/subjektiv/consolidation", self.workspace_id),
                serde_json::to_string(&MemoryConsolidateStagingRequest {
                    force: operation.force,
                })
                .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
            ))
            .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?;
        let response: MemoryConsolidationResponse = decode(response)?;
        Ok(MemoryConsolidationOutput {
            status: response.status,
            summary: response.summary,
            candidate_count: response.candidate_count,
            total_bytes: response.total_bytes,
        })
    }
}

/// Authenticated Backend Journal Job adapter, issued only from an explicit Host binding.
/// The Backend resolves Subject, Journal, generation and current attempt from that
/// authenticated Worker; no identity is copied into model input or request bodies.
pub struct BackendJournalJobHost {
    client: Arc<dyn WorkspaceClient>,
    workspace_id: String,
}
impl BackendJournalJobHost {
    pub fn new(
        client: Arc<dyn WorkspaceClient>,
        binding: &crate::BackendJobExecutionBinding,
    ) -> std::io::Result<Self> {
        binding.validate().map_err(std::io::Error::other)?;
        if !binding.subjektiv_journal || !client.is_available() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Journal Job requires explicit Host grant and available Backend authority",
            ));
        }
        let workspace_id = client
            .workspace_id()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Journal Job requires authenticated Workspace identity",
                )
            })?
            .to_string();
        Ok(Self {
            client,
            workspace_id,
        })
    }
    pub fn into_grant(self) -> crate::job::JobFeatureGrant {
        let host = Arc::new(self);
        crate::job::JobFeatureGrant {
            features: crate::feature::FeatureRegistryBuilder::new()
                .with_module(JournalJobToolsFeature::new(host.clone())),
            satisfied_requirements: vec![],
            finalizer: Some(Arc::new(JournalJobFinalizer::new(host))),
        }
    }
    fn post<T: serde::de::DeserializeOwned>(
        &self,
        suffix: &str,
        body: String,
    ) -> Result<T, SubjektivHostError> {
        decode(
            self.client
                .execute(WorkspaceRequest::json(
                    WorkspaceRequestMethod::Post,
                    format!("/api/w/{}/subjektiv/journal/job{suffix}", self.workspace_id),
                    body,
                ))
                .map_err(|error| SubjektivHostError::Unavailable(error.to_string()))?,
        )
    }
}
impl JournalJobHost for BackendJournalJobHost {
    fn execute(
        &self,
        request: transport::JournalJobRequest,
    ) -> Result<transport::JournalJobResponse, SubjektivHostError> {
        let body = serde_json::to_string(&request)
            .map_err(|error| SubjektivHostError::InvalidArgument(error.to_string()))?;
        self.post("", body)
    }
    fn finish(&self) -> Result<transport::JournalJobCompletion, SubjektivHostError> {
        self.post("/finish", "{}".into())
    }
}

/// Live connection with Worker-supplied authority. Not serializable and never
/// inherited by an ordinary SubWorker.
#[derive(Clone)]
pub struct SubjektivHostConnection {
    pub host: Arc<dyn SubjektivHost>,
    pub context: SubjektivHostContext,
}

impl SubjektivHostConnection {
    pub fn journal_startup_context(&self) -> Result<JournalStartupSnapshot, SubjektivHostError> {
        if !self.host.journal_startup_granted() {
            return Err(SubjektivHostError::Unavailable(
                "Journal startup not granted".into(),
            ));
        }
        self.host.journal_startup_context(&self.context)
    }

    pub fn journal_compaction_granted(&self) -> bool {
        self.host.journal_compaction_granted()
    }

    pub fn journal_compaction_context(&self) -> Result<JournalStartupSnapshot, SubjektivHostError> {
        if !self.host.journal_compaction_granted() {
            return Err(SubjektivHostError::Unavailable(
                "Journal compaction not granted".into(),
            ));
        }
        self.host.journal_compaction_context(&self.context)
    }
    pub fn new(
        host: Arc<dyn SubjektivHost>,
        context: SubjektivHostContext,
    ) -> std::io::Result<Self> {
        let language = host.settings().language;
        if context.worker_id.trim().is_empty()
            || context.session_id.trim().is_empty()
            || context.worker_id.chars().any(char::is_control)
            || context.session_id.chars().any(char::is_control)
            || !manifest::is_normalized_workspace_memory_language(&language)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid subjektiv Host context/settings",
            ));
        }
        Ok(Self { host, context })
    }
    pub fn memory_journal(
        &self,
        request: MemoryJournalRequest,
    ) -> Result<MemoryJournalResponse, SubjektivHostError> {
        self.host.memory_journal(&self.context, request)
    }
    pub fn memory(
        &self,
        operation: SubjektivMemoryBackendOperation,
    ) -> Result<SubjektivMemoryBackendResponse, SubjektivHostError> {
        if let SubjektivMemoryBackendOperation::StageExplicit(input) = &operation
            && input.session_id != self.context.session_id
        {
            return Err(SubjektivHostError::InvalidArgument(
                "explicit staging targets a foreign Session".into(),
            ));
        }
        if matches!(
            operation,
            SubjektivMemoryBackendOperation::DecideCandidate(_)
                | SubjektivMemoryBackendOperation::PrepareSurface(_)
                | SubjektivMemoryBackendOperation::PublishSurface(_)
                | SubjektivMemoryBackendOperation::FailSurface(_)
        ) && !self.host.consolidation_granted()
        {
            return Err(SubjektivHostError::InvalidArgument(
                "Host did not grant consolidation".into(),
            ));
        }
        self.host.memory(&self.context, operation)
    }
    pub fn session(
        &self,
        operation: SubjektivSessionBackendOperation,
    ) -> Result<SubjektivSessionBackendResponse, SubjektivHostError> {
        self.host.session(&self.context, operation)
    }
    pub async fn prepare_subjektiv_memory_surface(
        &self,
    ) -> Result<SubjektivSurfacePrepareResponse, SubjektivHostError> {
        match self.memory(SubjektivMemoryBackendOperation::PrepareSurface(
            SubjektivSurfacePrepareRequest {},
        ))? {
            SubjektivMemoryBackendResponse::SurfacePrepared(value) => Ok(value),
            _ => Err(SubjektivHostError::Unavailable(
                "unexpected surface preparation response".into(),
            )),
        }
    }
    pub async fn publish_subjektiv_memory_surface(
        &self,
        input: SubjektivSurfacePublishRequest,
    ) -> Result<SubjektivSurfacePublishResponse, SubjektivHostError> {
        match self.memory(SubjektivMemoryBackendOperation::PublishSurface(input))? {
            SubjektivMemoryBackendResponse::SurfacePublished(value) => Ok(value),
            _ => Err(SubjektivHostError::Unavailable(
                "unexpected surface publication response".into(),
            )),
        }
    }
    pub async fn fail_subjektiv_memory_surface(
        &self,
        input: SubjektivSurfaceFailureRequest,
    ) -> Result<SubjektivSurfaceFailureResponse, SubjektivHostError> {
        match self.memory(SubjektivMemoryBackendOperation::FailSurface(input))? {
            SubjektivMemoryBackendResponse::SurfaceFailed(value) => Ok(value),
            _ => Err(SubjektivHostError::Unavailable(
                "unexpected surface failure response".into(),
            )),
        }
    }
}

#[cfg(test)]
pub(crate) fn test_connection(client: Arc<dyn WorkspaceClient>) -> SubjektivHostConnection {
    let mut config = manifest::ResolvedSubjektivFeatureConfig::default();
    config.profile.enabled = true;
    config.profile.consolidation_tools = true;
    config
        .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: client.workspace_id().unwrap().to_string(),

            language: "English".into(),
        })
        .unwrap();
    SubjektivHostConnection::new(
        BackendSubjektivHost::from_resolved_config(&config, client)
            .unwrap()
            .unwrap(),
        SubjektivHostContext {
            worker_id: "worker-test".into(),
            session_id: "session-1".into(),
        },
    )
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Method, WorkerBootstrap, WorkerBootstrapLayout, WorkerControllerTransport,
        WorkerFilesystemAuthority, WorkerManifest, WorkerWorkspaceContext,
    };
    use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
    use agen::llm_client::{ClientError, LlmClient, Request};
    use async_trait::async_trait;
    use futures_util::Stream;
    use session_store::{CombinedStore, FsStore, FsWorkerStore, WorkerMetadataStore};
    use std::pin::Pin;
    use std::sync::Mutex;

    #[derive(Default)]
    struct LocalHost {
        startup: Option<JournalStartupSnapshot>,
        startup_error: bool,
        startup_calls: std::sync::atomic::AtomicUsize,
        contexts: Mutex<Vec<SubjektivHostContext>>,
        attributed: Mutex<Vec<(SubjektivHostContext, bool)>>,
        journal_observer:
            Option<Arc<dyn Fn(&SubjektivHostContext, JournalRecordingReason) + Send + Sync>>,
    }
    impl SubjektivHost for LocalHost {
        fn journal_startup_granted(&self) -> bool {
            self.startup.is_some() || self.startup_error
        }
        fn journal_startup_context(
            &self,
            _context: &SubjektivHostContext,
        ) -> Result<JournalStartupSnapshot, SubjektivHostError> {
            self.startup_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.startup_error {
                return Err(SubjektivHostError::Unavailable(
                    "startup read failed".into(),
                ));
            }
            self.startup
                .clone()
                .ok_or_else(|| SubjektivHostError::Unavailable("startup not granted".into()))
        }
        fn journal_recording_granted(&self) -> bool {
            self.journal_observer.is_some()
        }
        fn request_journal_recording(
            &self,
            context: &SubjektivHostContext,
            reason: JournalRecordingReason,
        ) -> Result<JournalRecordingResponse, SubjektivHostError> {
            if let Some(observer) = &self.journal_observer {
                observer(context, reason);
                Ok(JournalRecordingResponse::Accepted)
            } else {
                Err(SubjektivHostError::Unavailable(
                    "test Host did not grant Journal recording".into(),
                ))
            }
        }
        fn settings(&self) -> SubjektivHostSettings {
            SubjektivHostSettings {
                language: "English".into(),
            }
        }
        fn memory(
            &self,
            context: &SubjektivHostContext,
            operation: SubjektivMemoryBackendOperation,
        ) -> Result<SubjektivMemoryBackendResponse, SubjektivHostError> {
            self.contexts.lock().unwrap().push(context.clone());
            match operation {
                SubjektivMemoryBackendOperation::ResidentContext(_) => {
                    Ok(SubjektivMemoryBackendResponse::ResidentContext(
                        SubjektivResidentContextOutput {
                            behavior_md: "Explicit local subject behavior.".into(),
                            memory_surface: memory::backend::MemoryResidentSummaryOutput {
                                availability:
                                    memory::backend::MemoryResidentSummaryAvailability::Ungenerated,
                                content: None,
                            },
                        },
                    ))
                }
                _ => Err(SubjektivHostError::Unavailable(
                    "not granted by test Host".into(),
                )),
            }
        }
        fn session(
            &self,
            _: &SubjektivHostContext,
            _: SubjektivSessionBackendOperation,
        ) -> Result<SubjektivSessionBackendResponse, SubjektivHostError> {
            Err(SubjektivHostError::Unavailable("not used".into()))
        }
        fn stage_candidate(
            &self,
            _: &SubjektivHostContext,
            _: MemoryStageCandidateOperation,
        ) -> Result<SubjektivStageCandidateResponse, SubjektivHostError> {
            Err(SubjektivHostError::Unavailable("not used".into()))
        }
        fn record_session(
            &self,
            context: &SubjektivHostContext,
            create_if_missing: bool,
        ) -> Result<SubjektivRecordSessionResponse, SubjektivHostError> {
            self.attributed
                .lock()
                .unwrap()
                .push((context.clone(), create_if_missing));
            Ok(SubjektivRecordSessionResponse {
                subject_id: "test-owned-local-subject".into(),
                session_id: context.session_id.clone(),
            })
        }
        fn request_consolidation(
            &self,
            _: &SubjektivHostContext,
            _: MemoryConsolidateStagingOperation,
        ) -> Result<MemoryConsolidationOutput, SubjektivHostError> {
            Err(SubjektivHostError::Unavailable("no candidates".into()))
        }
    }

    #[derive(Clone)]
    struct ScriptedClient(tokio::sync::mpsc::UnboundedSender<Request>);
    #[async_trait]
    impl LlmClient for ScriptedClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
        async fn stream(
            &self,
            request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            self.0.send(request).unwrap();
            Ok(Box::pin(futures_util::stream::iter(vec![
                Ok(LlmEvent::text_block_start(0)),
                Ok(LlmEvent::text_delta(0, "Done.")),
                Ok(LlmEvent::text_block_stop(0, None)),
                Ok(LlmEvent::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                })),
            ])))
        }
    }

    #[derive(Clone, Default)]
    struct TaskScriptClient(Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait]
    impl LlmClient for TaskScriptClient {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
        async fn stream(
            &self,
            _: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            let step = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let call = match step {
                0 => Some((
                    "create",
                    "TaskCreate",
                    r#"{"subject":"verify boundary","description":"persist before admission"}"#,
                )),
                1 => Some((
                    "failed",
                    "TaskUpdate",
                    r#"{"taskid":999,"status":"completed"}"#,
                )),
                2 => Some((
                    "progress",
                    "TaskUpdate",
                    r#"{"taskid":1,"status":"inprogress"}"#,
                )),
                3 => Some((
                    "complete",
                    "TaskUpdate",
                    r#"{"taskid":1,"status":"completed"}"#,
                )),
                _ => None,
            };
            if step > 4 {
                return Err(ClientError::Config("scripted interruption".into()));
            }
            let mut events = if let Some((id, name, input)) = call {
                vec![
                    LlmEvent::tool_use_start(0, id, name),
                    LlmEvent::tool_input_delta(0, input),
                    LlmEvent::tool_use_stop(0),
                ]
            } else {
                vec![
                    LlmEvent::text_block_start(0),
                    LlmEvent::text_delta(0, "Done."),
                    LlmEvent::text_block_stop(0, None),
                ]
            };
            events.push(LlmEvent::Status(StatusEvent {
                status: ResponseStatus::Completed,
            }));
            Ok(Box::pin(futures_util::stream::iter(
                events.into_iter().map(Ok),
            )))
        }
    }

    // Integration boundary: real Bootstrap/controller, Task tools, Agen, annotation writer,
    // and FsStore. Only the external Journal Host and LLM transport are replaced.
    #[tokio::test]
    async fn journal_task_admission_observes_saved_success_before_run_finishes_and_each_run_requests()
     {
        use session_store::{LogEntry, LoggedItem, Store};
        let temp = tempfile::tempdir().unwrap();
        let _sandbox =
            crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
        let mut manifest = WorkerManifest::from_toml(&format!(
            r#"
[worker]
name = "journal-boundary-{}"
[model]
scheme = "anthropic"
model_id = "scripted-model"
[engine]
[scope]
allow = []
[feature.task]
enabled = true
"#,
            uuid::Uuid::now_v7()
        ))
        .unwrap();
        manifest.feature.subjektiv.profile.enabled = true;
        manifest.feature.subjektiv.profile.extraction.enabled = false;
        let store = CombinedStore::new(
            FsStore::new(temp.path().join("sessions")).unwrap(),
            FsWorkerStore::new(temp.path().join("workers")).unwrap(),
        );
        let read_store = store.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let host = Arc::new(LocalHost {
            journal_observer: Some(Arc::new(move |context, reason| {
                let session_id = context.session_id.parse().unwrap();
                let logs = read_store
                    .list_segments(session_id)
                    .unwrap()
                    .into_iter()
                    .flat_map(|segment| read_store.read_all(session_id, segment).unwrap())
                    .collect::<Vec<_>>();
                let complete_saved = logs.iter().any(|entry| matches!(entry,
                    LogEntry::AnnotatedToolResult { entry, .. }
                    if matches!(&entry.item, LoggedItem::ToolResult { call_id, is_error: false, .. } if call_id == "complete")));
                tx.send((reason, complete_saved)).unwrap();
            })),
            ..LocalHost::default()
        });
        let prepared = WorkerBootstrap::new(
            manifest,
            store,
            crate::PromptCatalogSource::builtins_only(),
            WorkerWorkspaceContext::local_filesystem(None),
            WorkerFilesystemAuthority::None,
            WorkerBootstrapLayout::Direct {
                runtime_base: temp.path().join("runtime"),
            },
            WorkerControllerTransport::InProcess,
        )
        .with_model_client(TaskScriptClient::default())
        .with_subjektiv_host(host)
        .prepare()
        .await
        .unwrap();
        let controller = prepared.start().await.unwrap();
        controller
            .handle
            .send(Method::submit_text(
                protocol::new_submission_request_id(),
                "Use the task tools.",
            ))
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            first,
            (JournalRecordingReason::TaskCompleted, true),
            "Task failed/inprogress must not request admission; completed result must already be saved"
        );
        let second = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second, (JournalRecordingReason::RunCommitted, true));
        controller
            .handle
            .send(Method::submit_text(
                protocol::new_submission_request_id(),
                "Short interrupted turn.",
            ))
            .await
            .unwrap();
        let third = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            third.0,
            JournalRecordingReason::RunCommitted,
            "interrupted short runs also request Host reconciliation"
        );
        controller
            .handle
            .send(Method::Shutdown {
                command: protocol::WorkerCommandEnvelope::new(1),
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), controller.shutdown)
            .await
            .unwrap()
            .unwrap();
        controller.controller_task.await.unwrap();
    }

    #[tokio::test]
    async fn journal_boundaries_request_without_threshold_and_only_compact_rewrite_requests() {
        use crate::hook::*;
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let observed = reasons.clone();
        let host = Arc::new(LocalHost {
            journal_observer: Some(Arc::new(move |_, reason| {
                observed.lock().unwrap().push(reason);
            })),
            ..LocalHost::default()
        });
        let connection = SubjektivHostConnection::new(
            host,
            SubjektivHostContext {
                worker_id: "worker".into(),
                session_id: "session".into(),
            },
        )
        .unwrap();
        let feature = JournalRecordingFeature::new(connection).unwrap();
        for exit in [
            RunCommittedExit::Finished,
            RunCommittedExit::Interrupted,
            RunCommittedExit::Paused,
            RunCommittedExit::Yielded,
        ] {
            <JournalRecordingFeature as Hook<RunCommitted>>::call(
                &feature,
                &RunCommittedContext {
                    invocation: HookInvocationContext::default(),
                    exit,
                    committed_history: HookHistoryRange::default(),
                },
            )
            .await
            .unwrap();
        }
        for kind in [
            SessionRewriteKind::Rewind,
            SessionRewriteKind::Compact,
            SessionRewriteKind::Fork,
            SessionRewriteKind::Restore,
        ] {
            assert_eq!(
                <JournalRecordingFeature as Hook<BeforeSessionRewrite>>::call(
                    &feature,
                    &BeforeSessionRewriteContext {
                        invocation: HookInvocationContext::default(),
                        kind,
                        current_history: HookHistoryRange::default(),
                    }
                )
                .await
                .unwrap(),
                BeforeSessionRewriteAction::Continue
            );
        }
        <JournalRecordingFeature as Hook<WorkerRestored>>::call(
            &feature,
            &WorkerRestoredContext::new(
                HookInvocationContext::default(),
                RestoreSystemItemAppendHandle::new(Arc::default()),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            *reasons.lock().unwrap(),
            vec![
                JournalRecordingReason::RunCommitted,
                JournalRecordingReason::RunCommitted,
                JournalRecordingReason::RunCommitted,
                JournalRecordingReason::RunCommitted,
                JournalRecordingReason::BeforeCompact,
                JournalRecordingReason::WorkerRestored,
            ]
        );
    }

    // Production Direct bootstrap, Feature selection/rendering, Core, FsStore,
    // and model Request. The authenticated Host boundary and provider are fake;
    // real Backend/standalone authority and live continuation are not claimed.
    #[tokio::test]
    async fn direct_startup_journal_is_durable_before_first_user_and_provider_exactly_once() {
        use session_store::{LogEntry, LoggedSessionHistoryOrigin, Store, SystemItem};
        use std::sync::atomic::Ordering;
        for (empty, omitted) in [(false, false), (true, false), (true, true)] {
            let contributes_context = !empty || omitted;
            let temp = tempfile::tempdir().unwrap();
            let _sandbox =
                crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
            let name = format!("startup-{}", uuid::Uuid::now_v7());
            let mut manifest = WorkerManifest::from_toml(&format!("[worker]\nname='{name}'\n[model]\nscheme='anthropic'\nmodel_id='scripted'\n[engine]\n[scope]\nallow=[]")).unwrap();
            manifest.feature.subjektiv.profile.enabled = true;
            manifest.feature.subjektiv.profile.extraction.enabled = false;
            let snapshot = JournalStartupSnapshot {
                subject_id: "subject".into(),
                omitted,
                journals: if empty {
                    vec![]
                } else {
                    vec![subjektiv::journal::startup::StartupJournal {
                        journal_id: "journal".into(),
                        source_session_id: "previous-session".into(),
                        state_token: "consistent-token".into(),
                        process_order: 4,
                        process_byte: 0,
                        current_position: "B adopted after correcting A; implementation unfinished"
                            .into(),
                        entries: vec![],
                        omitted: true,
                    }]
                },
            };
            let host = Arc::new(LocalHost {
                startup: Some(snapshot.clone()),
                ..LocalHost::default()
            });
            let store = CombinedStore::new(
                FsStore::new(temp.path().join("sessions")).unwrap(),
                FsWorkerStore::new(temp.path().join("workers")).unwrap(),
            );
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let prepared = WorkerBootstrap::new(
                manifest,
                store.clone(),
                crate::PromptCatalogSource::builtins_only(),
                WorkerWorkspaceContext::local_filesystem(None),
                WorkerFilesystemAuthority::None,
                WorkerBootstrapLayout::Direct {
                    runtime_base: temp.path().join("runtime"),
                },
                WorkerControllerTransport::InProcess,
            )
            .with_model_client(ScriptedClient(tx))
            .with_subjektiv_host(host.clone())
            .prepare()
            .await
            .unwrap();
            let (session, segment) = (
                prepared.worker().session_id(),
                prepared.worker().segment_id(),
            );
            let controller = prepared.start().await.unwrap();
            let initial = store.read_all(session, segment).unwrap();
            let journal = initial.iter().find_map(|entry| match entry {
                LogEntry::AnnotatedSystemItem { entry, .. }
                    if matches!(entry.item, SystemItem::JournalStartup { .. }) =>
                {
                    Some(entry)
                }
                _ => None,
            });
            assert_eq!(journal.is_some(), contributes_context);
            let body = journal.map(|journal| {
                assert_eq!(
                    journal.metadata.origin,
                    LoggedSessionHistoryOrigin::JournalContext
                );
                let SystemItem::JournalStartup {
                    body,
                    snapshot: saved,
                    snapshot_sha256,
                    prompt_provenance,
                } = &journal.item
                else {
                    unreachable!()
                };
                assert_eq!(*saved, serde_json::to_value(&snapshot).unwrap());
                use sha2::{Digest, Sha256};
                assert_eq!(
                    *snapshot_sha256,
                    Sha256::digest(serde_json::to_vec(saved).unwrap())
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                );
                assert_eq!(
                    prompt_provenance.logical_name,
                    "internal.subjektiv_journal_startup"
                );
                assert_eq!(
                    body.trim_end(),
                    format!(
                        "## Prior Journal context\n\n```json\n{}\n```",
                        serde_json::to_string(saved).unwrap()
                    )
                );
                body.as_str()
            });
            assert!(
                !initial
                    .iter()
                    .any(|e| matches!(e, LogEntry::AnnotatedUserInput { .. }))
            );
            let mut events = controller.handle.subscribe();
            for user in ["FIRST_USER_REQUEST", "SECOND_USER_REQUEST"] {
                controller
                    .handle
                    .send(Method::submit_text(
                        protocol::new_submission_request_id(),
                        user,
                    ))
                    .await
                    .unwrap();
                let request = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
                if let Some(body) = body {
                    let index = request
                        .items
                        .iter()
                        .position(|item| item.as_text() == Some(body))
                        .unwrap();
                    let user_index = request
                        .items
                        .iter()
                        .position(|item| item.as_text() == Some("FIRST_USER_REQUEST"))
                        .unwrap();
                    assert!(index < user_index);
                    assert!(matches!(
                        request.items[index],
                        agen::Item::Message {
                            role: agen::llm_client::types::Role::System,
                            ..
                        }
                    ));
                    assert_eq!(
                        request
                            .items
                            .iter()
                            .filter(|item| item.as_text() == Some(body))
                            .count(),
                        1
                    );
                } else {
                    assert!(!request.items.iter().any(|item| {
                        item.as_text()
                            .is_some_and(|text| text.contains("Prior Journal context"))
                    }));
                }
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        if matches!(events.recv().await.unwrap(), protocol::Event::RunEnd { .. }) {
                            break;
                        }
                    }
                })
                .await
                .unwrap();
            }
            assert_eq!(host.startup_calls.load(Ordering::SeqCst), 1);
            let logs = store.read_all(session, segment).unwrap();
            assert_eq!(logs.iter().filter(|entry| matches!(entry, LogEntry::AnnotatedSystemItem { entry, .. } if matches!(entry.item, SystemItem::JournalStartup { .. }))).count(), usize::from(contributes_context));
            controller
                .handle
                .send(Method::Shutdown {
                    command: protocol::WorkerCommandEnvelope::new(1),
                })
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), controller.shutdown)
                .await
                .unwrap()
                .unwrap();
            controller.controller_task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn direct_startup_read_error_rejects_exposure_without_initialized_head() {
        use session_store::Store;
        let temp = tempfile::tempdir().unwrap();
        let _sandbox =
            crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
        let mut manifest = WorkerManifest::from_toml(&format!("[worker]\nname='startup-error-{}'\n[model]\nscheme='anthropic'\nmodel_id='scripted'\n[engine]\n[scope]\nallow=[]", uuid::Uuid::now_v7())).unwrap();
        manifest.feature.subjektiv.profile.enabled = true;
        manifest.feature.subjektiv.profile.extraction.enabled = false;
        let store = CombinedStore::new(
            FsStore::new(temp.path().join("sessions")).unwrap(),
            FsWorkerStore::new(temp.path().join("workers")).unwrap(),
        );
        let host = Arc::new(LocalHost {
            startup_error: true,
            ..LocalHost::default()
        });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let prepared = WorkerBootstrap::new(
            manifest,
            store.clone(),
            crate::PromptCatalogSource::builtins_only(),
            WorkerWorkspaceContext::local_filesystem(None),
            WorkerFilesystemAuthority::None,
            WorkerBootstrapLayout::Direct {
                runtime_base: temp.path().join("runtime"),
            },
            WorkerControllerTransport::InProcess,
        )
        .with_model_client(ScriptedClient(tx))
        .with_subjektiv_host(host.clone())
        .prepare()
        .await
        .unwrap();
        let (session, segment) = (
            prepared.worker().session_id(),
            prepared.worker().segment_id(),
        );
        assert!(prepared.start().await.is_err());
        assert!(!store.exists(session, segment).unwrap());
        assert!(rx.try_recv().is_err());
        assert_eq!(
            host.startup_calls.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    async fn launch_case(enabled: bool, attach: bool) {
        let temp = tempfile::tempdir().unwrap();
        let _runtime_sandbox =
            crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
        let name = format!("subjektiv-host-{}", uuid::Uuid::now_v7());
        let mut manifest = WorkerManifest::from_toml(&format!(
            r#"
[worker]
name = "{name}"
[model]
scheme = "anthropic"
model_id = "scripted-model"
[engine]
[scope]
allow = []
[feature.task]
enabled = true
"#
        ))
        .unwrap();
        manifest.feature.subjektiv.profile.enabled = enabled;
        manifest.feature.subjektiv.profile.extraction.enabled = true;
        manifest.feature.subjektiv.profile.extraction.threshold = Some(u64::MAX);
        assert!(manifest.feature.subjektiv.workspace_settings.is_none());
        let store = CombinedStore::new(
            FsStore::new(temp.path().join("sessions")).unwrap(),
            FsWorkerStore::new(temp.path().join("workers")).unwrap(),
        );
        let host = Arc::new(LocalHost::default());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let bootstrap = WorkerBootstrap::new(
            manifest,
            store.clone(),
            crate::PromptCatalogSource::builtins_only(),
            WorkerWorkspaceContext::local_filesystem(None),
            WorkerFilesystemAuthority::None,
            WorkerBootstrapLayout::Direct {
                runtime_base: temp.path().join("runtime"),
            },
            WorkerControllerTransport::InProcess,
        )
        .with_model_client(ScriptedClient(tx));
        let bootstrap = if attach {
            bootstrap.with_subjektiv_host(host.clone())
        } else {
            bootstrap
        };
        let prepared = bootstrap.prepare().await.unwrap();
        let expected = SubjektivHostContext {
            worker_id: name.clone(),
            session_id: prepared.worker().session_id().to_string(),
        };
        assert!(
            prepared
                .worker()
                .manifest()
                .feature
                .subjektiv
                .workspace_settings
                .is_none()
        );
        assert!(prepared.worker().workspace_id().is_none());
        assert!(
            prepared
                .worker()
                .workspace_client()
                .workspace_id()
                .is_none()
        );
        let controller = prepared.start().await.unwrap();
        controller
            .handle
            .send(Method::submit_text(
                protocol::new_submission_request_id(),
                "Hello",
            ))
            .await
            .unwrap();
        let request = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let tools = request
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert!(
            tools.contains(&"TaskCreate"),
            "ordinary Features must survive"
        );
        for name in [
            "SubjektivMemoryQuery",
            "SubjektivMemoryRead",
            "SubjektivMemoryRemember",
            "SubjektivSessionList",
            "SubjektivSessionRead",
        ] {
            assert_eq!(tools.contains(&name), enabled && attach, "{name}");
        }
        assert!(!tools.contains(&"MemoryApplyCandidate"));
        let rendered = format!(
            "{}\n{}",
            request.system_prompt.as_deref().unwrap_or_default(),
            request
                .items
                .iter()
                .filter_map(|item| item.as_text())
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert_eq!(
            rendered.contains("Explicit local subject behavior."),
            enabled && attach
        );
        if enabled && attach {
            assert_eq!(
                *host.attributed.lock().unwrap(),
                vec![(expected.clone(), true), (expected.clone(), false)]
            );
            assert!(
                host.contexts
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|context| context == &expected)
            );
            let metadata = store.read_by_name(&name).unwrap().unwrap();
            assert!(matches!(
                metadata.subjektiv_session_attribution,
                Some(session_store::SubjektivSessionAttributionState::Confirmed { .. })
            ));
        } else {
            assert!(host.contexts.lock().unwrap().is_empty());
            assert!(host.attributed.lock().unwrap().is_empty());
        }
        controller
            .handle
            .send(Method::Shutdown {
                command: protocol::WorkerCommandEnvelope::new(1),
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), controller.shutdown)
            .await
            .unwrap()
            .unwrap();
        controller.controller_task.await.unwrap();
    }

    #[tokio::test]
    async fn explicit_local_host_installs_ordinary_features_without_workspace_snapshot() {
        launch_case(true, true).await;
    }
    #[tokio::test]
    async fn enabled_policy_without_host_is_inert() {
        launch_case(true, false).await;
    }
    #[tokio::test]
    async fn disabled_policy_with_host_has_no_subjektiv_side_effects() {
        launch_case(false, true).await;
    }
    #[test]
    fn consolidation_installation_requires_separate_explicit_host_grant() {
        let connection = SubjektivHostConnection::new(
            Arc::new(LocalHost::default()),
            SubjektivHostContext {
                worker_id: "test-worker".into(),
                session_id: "test-session".into(),
            },
        )
        .unwrap();
        assert!(SubjektivConsolidationToolsFeature::new(connection).is_err());
    }
}
