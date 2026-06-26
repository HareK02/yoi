//! Worker プロトコルを喋るクライアント。
//!
//! - [`WorkerClient`]: 既存 worker の Unix ソケットへ接続して `Method` を送り、
//!   `Event` を受け取る低レベル接続。
//! - [`spawn`]: worker バイナリをサブプロセスとして起動し、`YOI-READY`
//!   ハンドシェイクが終わるまで待つフロー。subprocess を立ち上げる必要が
//!   ない呼び出し側 (=既存 worker に attach する場合) は使わなくてよい。
//!
//! TUI / GUI / E2E ハーネスはこの crate に依存して protocol を喋る。

pub mod backend_runtime;
pub mod runtime_command;
pub mod spawn;
pub mod ticket_role;
mod worker_client;

pub use backend_runtime::{BackendRuntimeClient, BackendRuntimeClientError, BackendRuntimeTarget};
pub use runtime_command::WorkerRuntimeCommand;

pub use spawn::{
    SpawnConfig, SpawnError, SpawnReady, WorkerProcessLaunchConfig, WorkerProcessLaunchOptions,
    spawn_worker, spawn_worker_with_options,
};
pub use ticket_role::{
    TicketRef, TicketRoleLaunchContext, TicketRoleLaunchError, TicketRoleLaunchOptions,
    TicketRoleLaunchPlan, TicketRoleLaunchResult, TicketRolePreRunWarning,
    launch_ticket_role_worker, launch_ticket_role_worker_with_options, plan_ticket_role_launch,
    plan_ticket_role_launch_with_config,
};
pub use worker_client::WorkerClient;
