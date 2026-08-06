//! `WorkerEvent` send / receive helpers.
//!
//! This module owns the parent-facing lifecycle-event primitive
//! (`WorkerEvent`) that children fire upward on turn-end / error /
//! shutdown / scope-sub-delegation. Three responsibilities live here:
//!
//! - **Send** a `Method::WorkerEvent` to the parent socket, fire-and-forget,
//!   logging failures without blocking the child.
//! - **Render** agent-visible variants into human-readable strings for the
//!   parent's notification buffer. Control-plane-only variants may still have
//!   a renderer for diagnostics, but receive-side classification keeps them
//!   out of LLM history/context.
//! - **Apply side effects** on the parent (registry / worker-allocation
//!   updates) so that the receive path is idempotent and tolerant of
//!   out-of-order delivery.
//!
//! Transport is fire-and-forget — the ticket's decision is that
//! callbacks are an optimisation and `ListWorkers` + `reclaim_stale` are
//! the real fallback. This module is allowed to drop events on the
//! floor (with a warn log) rather than retry.
//!
//! `apply_event_side_effects` takes its dependencies (registry, scope
//! lock path, self identity) by reference so the caller owns lifetime
//! and locking concerns.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use protocol::{Method, WorkerEvent};

use crate::spawn::comm_tools::connect_and_send;
use crate::spawn::registry::SpawnedWorkerRegistry;

/// Connect to `socket`, send a single `Method::WorkerEvent(event)`, and
/// return. Used by children to report up to their parent.
///
/// This is a synchronous helper — callers that want fire-and-forget
/// semantics should wrap the call in `tokio::spawn` themselves.
pub async fn send_worker_event(socket: &Path, event: WorkerEvent) -> std::io::Result<()> {
    connect_and_send(socket, &Method::WorkerEvent(event)).await
}

/// Spawn a fire-and-forget task that sends `event` to `socket`. If
/// `socket` is `None`, no send happens (top-level Workers have no parent).
/// Any send failure is logged at warn level but otherwise ignored —
/// the parent is treated as best-effort.
pub fn fire_and_forget(socket: Option<PathBuf>, event: WorkerEvent) {
    let Some(socket) = socket else { return };
    tokio::spawn(async move {
        if let Err(e) = send_worker_event(&socket, event).await {
            tracing::warn!(error = %e, socket = %socket.display(), "WorkerEvent send failed");
        }
    });
}

/// Render a variant into a one-line human-readable string.
///
/// Only events classified by `WorkerEvent::should_notify_agent` are injected
/// into the parent's LLM context as system messages; control-plane-only events
/// keep this renderer for diagnostics/tests. Agent-visible summaries are kept
/// deliberately short — the LLM can always call `SubWorkerReadOutput` to fetch more
/// detail if the event summary is not enough.
pub fn render_event(event: &WorkerEvent) -> String {
    match event {
        WorkerEvent::TurnEnded { worker_name } => {
            format!("Worker `{worker_name}` finished a turn.")
        }
        WorkerEvent::Errored {
            worker_name,
            message,
        } => {
            format!("Worker `{worker_name}` reported an error: {message}")
        }
        WorkerEvent::ShutDown { worker_name } => {
            format!("Worker `{worker_name}` has stopped.")
        }
        WorkerEvent::ScopeSubDelegated {
            parent_worker,
            sub_worker,
            ..
        } => {
            format!("Worker `{parent_worker}` spawned `{sub_worker}` and delegated scope to it.")
        }
    }
}

/// Legacy process callback events have no SubWorker registry authority.
///
/// Internal SubWorker lifecycle is applied directly through typed session handles. A callback from
/// an externally adopted Worker may still be rendered for diagnostics, but it cannot add/remove
/// Internal children or transfer filesystem authority.
pub async fn apply_event_side_effects(
    _event: &WorkerEvent,
    _registry: &Arc<SpawnedWorkerRegistry>,
    _self_name: &str,
    _self_parent_socket: &Option<PathBuf>,
) {
}
