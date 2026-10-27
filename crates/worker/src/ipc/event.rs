//! Agent-visible rendering of typed Internal SubWorker lifecycle events.

use protocol::WorkerEvent;

/// Render a variant into a one-line human-readable string.
///
/// Only events classified by `WorkerEvent::should_notify_agent` are injected
/// into the parent's LLM context as system messages; control-plane-only events
/// keep this renderer for diagnostics/tests. Agent-visible summaries are kept
/// deliberately short — the LLM can use worker-observation tools to inspect the committed
/// session when the event summary is not enough.
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
