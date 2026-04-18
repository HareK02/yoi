//! `PodEvent` send / receive helpers.
//!
//! This module owns the parent-facing lifecycle-event primitive
//! (`PodEvent`) that children fire upward on turn-end / error /
//! shutdown / scope-sub-delegation. Three responsibilities live here:
//!
//! - **Send** a `Method::PodEvent` to the parent socket, fire-and-forget,
//!   logging failures without blocking the child.
//! - **Render** a variant into a human-readable string that the parent's
//!   LLM sees via the notification buffer.
//! - **Apply side effects** on the parent (registry / scope-lock
//!   updates) so that the receive path is idempotent and tolerant of
//!   out-of-order delivery.
//!
//! Transport is fire-and-forget — the ticket's decision is that
//! callbacks are an optimisation and `ListPods` + `reclaim_stale` are
//! the real fallback. This module is allowed to drop events on the
//! floor (with a warn log) rather than retry.
//!
//! `apply_event_side_effects` takes its dependencies (registry, scope
//! lock path, self identity) by reference so the caller owns lifetime
//! and locking concerns.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use protocol::{Method, PodEvent, ScopeRule};

use crate::pod_comm_tools::connect_and_send;
use crate::runtime_dir::SpawnedPodRecord;
use crate::scope_lock::{self, ScopeLockError};
use crate::spawned_pod_registry::SpawnedPodRegistry;

/// Connect to `socket`, send a single `Method::PodEvent(event)`, and
/// return. Used by children to report up to their parent.
///
/// This is a synchronous helper — callers that want fire-and-forget
/// semantics should wrap the call in `tokio::spawn` themselves.
pub async fn send_pod_event(socket: &Path, event: PodEvent) -> std::io::Result<()> {
    connect_and_send(socket, &Method::PodEvent(event)).await
}

/// Spawn a fire-and-forget task that sends `event` to `socket`. If
/// `socket` is `None`, no send happens (top-level Pods have no parent).
/// Any send failure is logged at warn level but otherwise ignored —
/// the parent is treated as best-effort.
pub fn fire_and_forget(socket: Option<PathBuf>, event: PodEvent) {
    let Some(socket) = socket else { return };
    tokio::spawn(async move {
        if let Err(e) = send_pod_event(&socket, event).await {
            tracing::warn!(error = %e, socket = %socket.display(), "PodEvent send failed");
        }
    });
}

/// Render a variant into the one-line human-readable string that will
/// be injected into the parent's LLM context as a system message.
///
/// Kept deliberately short — the LLM can always call `ReadPodOutput`
/// to fetch more detail if the event summary is not enough.
pub fn render_event(event: &PodEvent) -> String {
    match event {
        PodEvent::TurnEnded { pod_name } => {
            format!("Pod `{pod_name}` finished a turn.")
        }
        PodEvent::Errored { pod_name, message } => {
            format!("Pod `{pod_name}` reported an error: {message}")
        }
        PodEvent::ShutDown { pod_name } => {
            format!("Pod `{pod_name}` has stopped.")
        }
        PodEvent::ScopeSubDelegated {
            parent_pod,
            sub_pod,
            ..
        } => {
            format!("Pod `{parent_pod}` spawned `{sub_pod}` and delegated scope to it.")
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
/// - `ShutDown`: remove the child from `spawned_pods.json` and release
///   its scope allocation. Missing entries are swallowed.
/// - `ScopeSubDelegated`: register the grandchild locally and re-emit
///   upward to our own parent if we have one. Duplicate grandchild
///   entries (re-delivery) are swallowed.
pub async fn apply_event_side_effects(
    event: &PodEvent,
    registry: &Arc<SpawnedPodRegistry>,
    self_name: &str,
    self_parent_socket: &Option<PathBuf>,
) {
    match event {
        PodEvent::TurnEnded { .. } | PodEvent::Errored { .. } => {}

        PodEvent::ShutDown { pod_name } => {
            if let Err(e) = registry.remove(pod_name).await {
                tracing::warn!(error = %e, pod = %pod_name, "registry remove on ShutDown failed");
            }
            release_scope_silently(pod_name);
        }

        PodEvent::ScopeSubDelegated {
            parent_pod,
            sub_pod,
            sub_socket,
            scope,
        } => {
            if registry.get(sub_pod).await.is_some() {
                return;
            }
            let callback_address = registry
                .get(parent_pod)
                .await
                .map(|r| r.socket_path)
                .unwrap_or_else(PathBuf::new);
            let record = SpawnedPodRecord {
                pod_name: sub_pod.clone(),
                socket_path: sub_socket.clone(),
                scope_delegated: scope.clone(),
                callback_address,
            };
            if let Err(e) = registry.add(record).await {
                tracing::warn!(
                    error = %e,
                    sub_pod = %sub_pod,
                    "registry add on ScopeSubDelegated failed"
                );
            }
            reemit_scope_sub_delegated(
                self_parent_socket,
                self_name,
                sub_pod.clone(),
                sub_socket.clone(),
                scope.clone(),
            );
        }
    }
}

fn release_scope_silently(pod_name: &str) {
    let lock_path = match scope_lock::default_lock_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "default_lock_path failed");
            return;
        }
    };
    let mut guard = match scope_lock::LockFileGuard::open(&lock_path) {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(error = %e, "LockFileGuard open failed");
            return;
        }
    };
    match scope_lock::release_pod(&mut guard, pod_name) {
        Ok(()) => {}
        Err(ScopeLockError::UnknownPod(_)) => {}
        Err(e) => tracing::warn!(error = ?e, pod = %pod_name, "release_pod failed"),
    }
}

fn reemit_scope_sub_delegated(
    self_parent_socket: &Option<PathBuf>,
    self_name: &str,
    sub_pod: String,
    sub_socket: PathBuf,
    scope: Vec<ScopeRule>,
) {
    let Some(parent_socket) = self_parent_socket.clone() else {
        return;
    };
    let event = PodEvent::ScopeSubDelegated {
        parent_pod: self_name.to_string(),
        sub_pod,
        sub_socket,
        scope,
    };
    fire_and_forget(Some(parent_socket), event);
}
