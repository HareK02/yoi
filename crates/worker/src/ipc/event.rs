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

use protocol::{Method, ScopeRule, WorkerEvent};

use crate::runtime::dir::SpawnedWorkerRecord;
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

/// Apply the variant-specific side effect on the parent side.
///
/// All operations are idempotent so that out-of-order delivery (e.g.
/// `TurnEnded` arriving after `ShutDown`) does not produce errors:
///
/// - `TurnEnded` / `Errored`: no system work; the LLM handles the
///   semantic response.
/// - `ShutDown`: remove the child from `spawned_workers.json`, Worker state,
///   and reclaim its delegated scope/allocation. Missing entries are swallowed.
/// - `ScopeSubDelegated`: register the grandchild locally and re-emit
///   upward to our own parent if we have one. Duplicate grandchild
///   entries (re-delivery) are swallowed.
pub async fn apply_event_side_effects(
    event: &WorkerEvent,
    registry: &Arc<SpawnedWorkerRegistry>,
    self_name: &str,
    self_parent_socket: &Option<PathBuf>,
) {
    match event {
        WorkerEvent::TurnEnded { .. } | WorkerEvent::Errored { .. } => {}

        WorkerEvent::ShutDown { worker_name } => {
            if let Err(e) = registry.remove(worker_name).await {
                tracing::warn!(error = %e, worker = %worker_name, "registry remove on ShutDown failed");
            }
        }

        WorkerEvent::ScopeSubDelegated {
            parent_worker,
            sub_worker,
            sub_socket,
            scope,
        } => {
            if registry.get(sub_worker).await.is_some() {
                return;
            }
            let callback_address = registry
                .get(parent_worker)
                .await
                .map(|r| r.socket_path)
                .unwrap_or_else(PathBuf::new);
            let record = SpawnedWorkerRecord {
                worker_name: sub_worker.clone(),
                socket_path: sub_socket.clone(),
                scope_delegated: scope.clone(),
                callback_address,
            };
            if let Err(e) = registry.add(record).await {
                tracing::warn!(
                    error = %e,
                    sub_worker = %sub_worker,
                    "registry add on ScopeSubDelegated failed"
                );
            }
            reemit_scope_sub_delegated(
                self_parent_socket,
                self_name,
                sub_worker.clone(),
                sub_socket.clone(),
                scope.clone(),
            );
        }
    }
}

fn reemit_scope_sub_delegated(
    self_parent_socket: &Option<PathBuf>,
    self_name: &str,
    sub_worker: String,
    sub_socket: PathBuf,
    scope: Vec<ScopeRule>,
) {
    let Some(parent_socket) = self_parent_socket.clone() else {
        return;
    };
    let event = WorkerEvent::ScopeSubDelegated {
        parent_worker: self_name.to_string(),
        sub_worker,
        sub_socket,
        scope,
    };
    fire_and_forget(Some(parent_socket), event);
}
