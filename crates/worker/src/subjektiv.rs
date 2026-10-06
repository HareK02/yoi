//! Explicit Host capability for subject-scoped Memory and committed Sessions.
//!
//! A manifest is policy, not a connection. Hosts bind Subject/storage authority
//! in their implementation; the Worker supplies its own Worker/Session context.
//! Neither Subject identity nor filesystem/Workspace authority is model input.

pub use crate::feature::builtin::memory::SubjektivConsolidationToolsFeature;
pub use crate::feature::builtin::memory_surface_lifecycle::SubjektivSurfaceLifecycleFeature;

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
                        "revision_conflict"
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

/// Live connection with Worker-supplied authority. Not serializable and never
/// inherited by an ordinary SubWorker.
#[derive(Clone)]
pub struct SubjektivHostConnection {
    pub host: Arc<dyn SubjektivHost>,
    pub context: SubjektivHostContext,
}

impl SubjektivHostConnection {
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
            settings_revision: 1,
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
        contexts: Mutex<Vec<SubjektivHostContext>>,
        attributed: Mutex<Vec<(SubjektivHostContext, bool)>>,
    }
    impl SubjektivHost for LocalHost {
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
                            behavior_revision: 1,
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
                bash_output_dir: temp.path().join("bash-output"),
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
                vec![(expected.clone(), true)]
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
