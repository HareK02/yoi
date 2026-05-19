//! Typed system-message items injected by the agent system.
//!
//! Items in worker history with `role:system` are never produced by the
//! LLM — they are always inserted by the Pod itself (notifications,
//! file/knowledge/workflow ref resolutions, child-pod lifecycle events,
//! future `<system-reminder>` tags, …). [`SystemItem`] carries the
//! typed shape of each such injection so clients can dispatch on
//! `kind` instead of parsing text prefixes like `[Notification] …` or
//! `[File: …]`.
//!
//! Persisted as the payload of [`crate::LogEntry::SystemItem`] (one
//! entry per item), and broadcast live as the payload of
//! `Event::SystemItem` on the wire.
//!
//! For LLM context replay, each `SystemItem` reduces to an
//! `Item::system_message(...)` whose body matches the legacy free-text
//! shape (see [`SystemItem::history_text`]). The kind metadata is
//! preserved only on the log/wire side; the LLM still sees plain
//! system-message text.

use llm_worker::llm_client::types::Item;
use protocol::PodEvent;
use serde::{Deserialize, Serialize};

/// One agent-injected system item, tagged by origin.
///
/// Each variant carries the kind-specific raw data clients use for
/// typed rendering (`Notification.message`, `PodEvent.event`, file
/// path / knowledge slug / workflow slug / etc.), plus a pre-rendered
/// `body` (where applicable) that is the exact `role:system` text the
/// LLM actually saw at commit time. `body` is denormalised so that
/// segment log replay reconstructs worker history byte-identical to
/// what was on the wire — even when prompt overrides (e.g. custom
/// `notify_wrapper` template) re-shape the live rendering on a later
/// resume.
///
/// New variants get added here as fresh injection kinds come online
/// (e.g. `Reminder`). The `kind` JSON tag is the snake_case form of
/// the variant name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SystemItem {
    /// Free-form notification sent in by an external caller via
    /// `Method::Notify`. `message` is the raw caller-supplied text;
    /// `body` is the wrapped LLM-context form (Pod renders it via
    /// `notify_wrapper` at commit time).
    Notification { message: String, body: String },

    /// Lifecycle event reported by a child Pod via `Method::PodEvent`.
    /// `event` is the typed payload (so the TUI can render per-child
    /// banners without re-parsing); `body` is the wrapped LLM-context
    /// form (same `notify_wrapper` path as `Notification`).
    PodEvent { event: PodEvent, body: String },

    /// `@<path>` file reference resolution. `body` is the rendered
    /// LLM-context text (`[File: <path>]\n…` for regular files,
    /// `[Dir: <path>]\n…` for directory listings, possibly with a
    /// truncation hint) so replay reconstructs worker history
    /// byte-identical to what was sent.
    FileAttachment { path: String, body: String },

    /// `#<slug>` Knowledge reference resolution. `body` is the
    /// rendered text the LLM saw (Pod composes the `[Knowledge: …]`
    /// header + body).
    Knowledge { slug: String, body: String },

    /// `/<slug>` Workflow invocation. `body` is the workflow's
    /// prompt body materialized into the LLM context.
    Workflow { slug: String, body: String },
}

impl SystemItem {
    /// Free-text body the LLM sees inside its `role:system` message
    /// for this item. Returns the variant's stored `body` verbatim.
    pub fn history_text(&self) -> String {
        match self {
            SystemItem::Notification { body, .. } => body.clone(),
            SystemItem::PodEvent { body, .. } => body.clone(),
            SystemItem::FileAttachment { body, .. } => body.clone(),
            SystemItem::Knowledge { body, .. } => body.clone(),
            SystemItem::Workflow { body, .. } => body.clone(),
        }
    }

    /// Materialize this `SystemItem` as the `Item::system_message`
    /// form that lands in worker history.
    pub fn to_history_item(&self) -> Item {
        Item::system_message(self.history_text())
    }

    /// Short human-readable label used for diagnostics. Not on the
    /// wire — keep flexible.
    pub fn kind_label(&self) -> &'static str {
        match self {
            SystemItem::Notification { .. } => "notification",
            SystemItem::PodEvent { .. } => "pod_event",
            SystemItem::FileAttachment { .. } => "file_attachment",
            SystemItem::Knowledge { .. } => "knowledge",
            SystemItem::Workflow { .. } => "workflow",
        }
    }
}

/// Render a `PodEvent` as the one-line notification text the agent
/// sees. Centralised here (rather than at the controller's render
/// site) so persistence and broadcast share the same rendering.
pub fn render_pod_event(event: &PodEvent) -> String {
    match event {
        PodEvent::TurnEnded { pod_name } => format!("pod `{pod_name}` finished a turn"),
        PodEvent::Errored { pod_name, message } => {
            format!("pod `{pod_name}` errored: {message}")
        }
        PodEvent::ShutDown { pod_name } => format!("pod `{pod_name}` shut down"),
        PodEvent::ScopeSubDelegated {
            parent_pod,
            sub_pod,
            ..
        } => {
            format!("pod `{parent_pod}` sub-delegated scope to `{sub_pod}`")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_history_text_returns_stored_body() {
        let item = SystemItem::Notification {
            message: "child done".into(),
            body: "[Notification]\nchild done\n\n(non-blocking hint…)".into(),
        };
        assert_eq!(
            item.history_text(),
            "[Notification]\nchild done\n\n(non-blocking hint…)"
        );
    }

    #[test]
    fn pod_event_history_text_returns_stored_body() {
        let item = SystemItem::PodEvent {
            event: PodEvent::TurnEnded {
                pod_name: "child".into(),
            },
            body: "[Notification]\npod `child` finished a turn\n\n(non-blocking hint…)".into(),
        };
        assert!(item.history_text().starts_with("[Notification]\n"));
        assert!(item.history_text().contains("`child`"));
    }

    #[test]
    fn file_attachment_history_text_returns_stored_body() {
        let item = SystemItem::FileAttachment {
            path: "src/main.rs".into(),
            body: "[File: src/main.rs]\nfn main() {}".into(),
        };
        assert_eq!(item.history_text(), "[File: src/main.rs]\nfn main() {}");
    }

    #[test]
    fn round_trip_via_json() {
        let item = SystemItem::FileAttachment {
            path: "src/main.rs".into(),
            body: "[File: src/main.rs]\nfn main() {}".into(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: SystemItem = serde_json::from_str(&json).unwrap();
        match parsed {
            SystemItem::FileAttachment { path, body } => {
                assert_eq!(path, "src/main.rs");
                assert_eq!(body, "[File: src/main.rs]\nfn main() {}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn round_trip_pod_event() {
        let item = SystemItem::PodEvent {
            event: PodEvent::TurnEnded {
                pod_name: "child".into(),
            },
            body: "[Notification] pod `child` finished a turn".into(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: SystemItem = serde_json::from_str(&json).unwrap();
        match parsed {
            SystemItem::PodEvent {
                event: PodEvent::TurnEnded { pod_name },
                body,
            } => {
                assert_eq!(pod_name, "child");
                assert!(body.contains("`child`"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
