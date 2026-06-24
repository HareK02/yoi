//! Pod プロトコルを喋るクライアント。
//!
//! - [`PodClient`]: 既存 pod の Unix ソケットへ接続して `Method` を送り、
//!   `Event` を受け取る低レベル接続。
//! - [`spawn`]: pod バイナリをサブプロセスとして起動し、`YOI-READY`
//!   ハンドシェイクが終わるまで待つフロー。subprocess を立ち上げる必要が
//!   ない呼び出し側 (=既存 pod に attach する場合) は使わなくてよい。
//!
//! TUI / GUI / E2E ハーネスはこの crate に依存して protocol を喋る。

mod pod_client;
pub mod runtime_command;
pub mod spawn;
pub mod ticket_role;

pub use runtime_command::PodRuntimeCommand;

pub use pod_client::PodClient;
pub use spawn::{
    PodProcessLaunchConfig, PodProcessLaunchOptions, SpawnConfig, SpawnError, SpawnReady,
    spawn_pod, spawn_pod_with_options,
};
pub use ticket_role::{
    TicketRef, TicketRoleLaunchContext, TicketRoleLaunchError, TicketRoleLaunchOptions,
    TicketRoleLaunchPlan, TicketRoleLaunchResult, TicketRolePreRunWarning, launch_ticket_role_pod,
    launch_ticket_role_pod_with_options, plan_ticket_role_launch,
    plan_ticket_role_launch_with_config,
};
