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
pub mod segment_log_sink;
pub mod shared_state;
mod shutdown_after_idle;
pub mod skill;
pub mod spawn;

mod interrupt_prep;
mod permission;
mod ticket_event_notify;
mod worker;

pub use compact::token_counter::{EstimateSource, SplitPoint, TokenEstimate};
pub use controller::{ShutdownReceiver, WorkerController, WorkerHandle};
pub use hook::{Hook, HookEventKind, HookRegistryBuilder};
pub use ipc::alerter::Alerter;
pub use ipc::server::SocketServer;
pub use manifest::{
    AuthRef, ModelManifest, SchemeKind, Scope, WorkerManifest, WorkerManifestConfig,
    WorkerMetaConfig,
};
pub use model_client::{ProviderError, build_client};
pub use prompt::catalog::{CatalogError, PromptCatalog, WorkerPrompt};
pub use prompt::loader::PromptLoader;
pub use prompt::system::{SystemPromptContext, SystemPromptError, SystemPromptTemplate};
pub use protocol::{ErrorCode, Event, Method, TurnResult, WorkerStatus};
pub use runtime::dir::RuntimeDir;
pub use segment_log_sink::SegmentLogSink;
pub use shared_state::WorkerSharedState;
pub use worker::{
    LocalWorkingDirectory, Worker, WorkerError, WorkerFilesystemAuthority, WorkerRunResult,
    WorkerWorkspaceContext, WorkspaceClient, WorkspaceId, WorkspaceIdError, apply_worker_manifest,
};
