pub mod bootstrap;
pub mod compact;
pub mod controller;
pub mod discovery;
pub mod entrypoint;
pub mod feature;
pub mod fs_view;
pub mod hook;
pub(crate) mod in_flight;
pub mod ipc;
pub mod model_client;
pub mod prompt;
pub mod runtime;
pub mod runtime_command;
pub mod segment_log_sink;
mod session_capture;
mod session_history;
pub mod shared_state;
mod shutdown_after_idle;
pub mod skill;
pub mod spawn;

mod internal_worker;
mod interrupt_prep;
mod permission;
mod worker;

pub use bootstrap::{
    BootstrappedWorker, PreparedWorker, WorkerBootstrap, WorkerBootstrapError,
    WorkerBootstrapLayout, start_worker_controller,
};
pub use compact::token_counter::{EstimateSource, SplitPoint, TokenEstimate};
pub use controller::{ShutdownReceiver, WorkerController, WorkerControllerTransport, WorkerHandle};
pub use hook::{Hook, HookEventKind, HookRegistryBuilder};
pub use ipc::alerter::Alerter;
pub use ipc::server::SocketServer;
pub use manifest::{
    AuthRef, ModelManifest, SchemeKind, Scope, WorkerManifest, WorkerManifestConfig,
    WorkerMetaConfig,
};
pub use model_client::{ProviderError, build_client};
pub use prompt::catalog::{
    CatalogError, EffectivePromptCatalog, OrchestratorQueueAttentionContext,
    OrchestratorQueueAttentionPrompt, OrchestratorQueueAttentionTicket, PromptCatalog,
    WorkerPrompt, WorkspacePromptProjection, prompt_schema_source,
};
pub use prompt::source::PromptCatalogSource;
pub use prompt::system::{SystemPromptContext, SystemPromptError, SystemPromptTemplate};
pub use protocol::{ErrorCode, Event, Method, TurnResult, WorkerStatus};
pub use runtime::dir::RuntimeDir;
pub use segment_log_sink::SegmentLogSink;
pub use session_history::{
    SessionHistoryDerivation, SessionHistoryEntryId, SessionHistoryMetadata,
    WorkerHistoryProvenance, WorkerSubjectSnapshot,
};
pub use shared_state::WorkerSharedState;
pub use worker::{
    LocalWorkingDirectory, WORKER_INPUT_SUBMISSION_EXTENSION_DOMAIN, Worker, WorkerError,
    WorkerFilesystemAuthority, WorkerRunResult, WorkerWorkspaceContext, WorkspaceClient,
    WorkspaceClientError, WorkspaceId, WorkspaceIdError, WorkspacePromptCatalogResolution,
    WorkspaceRequest, WorkspaceRequestMethod, WorkspaceResponse, apply_worker_manifest,
    marker_workspace_client, unavailable_workspace_client,
};
