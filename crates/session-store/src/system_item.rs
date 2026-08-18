//! Typed system-message items injected by the agent system.
//!
//! Items in worker history with `role:system` are never produced by the
//! LLM — they are always inserted by the Worker itself (notifications,
//! file ref resolutions, child-worker lifecycle events,
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

use llm_engine::llm_client::types::Item;
use protocol::WorkerEvent;
use serde::{Deserialize, Serialize};

const SYSTEM_REMINDER_OPEN: &str = "<system-reminder>";
const SYSTEM_REMINDER_CLOSE: &str = "</system-reminder>";

/// Source policy that produced a durable `<system-reminder>` input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemReminderSource {
    /// Session task inactivity reminder emitted after task-management tools
    /// have gone unused for the configured request threshold.
    TaskInactivity,
}

fn default_task_reminder_source() -> SystemReminderSource {
    SystemReminderSource::TaskInactivity
}

/// Typed pending system reminder before it is committed as a [`SystemItem`].
///
/// System reminders are durable input: producers must append the rendered
/// `SystemItem` through worker history before the next LLM request. They are
/// not transient UI notices and must not be prompt-cache/context-only
/// injections, because then the model could react to input that is absent from
/// persisted history on later turns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemReminder {
    source: SystemReminderSource,
    body: String,
}

impl SystemReminder {
    /// Build a task-inactivity reminder from an unwrapped body.
    pub fn task_inactivity(body: impl Into<String>) -> Self {
        Self::new(SystemReminderSource::TaskInactivity, body)
    }

    /// Build a reminder from an unwrapped body. If a caller passes a body that
    /// is already exactly wrapped in `<system-reminder>` tags, normalize it back
    /// to the inner body so rendering still wraps exactly once.
    pub fn new(source: SystemReminderSource, body: impl Into<String>) -> Self {
        let body = normalize_unwrapped_system_reminder_body(body.into());
        Self { source, body }
    }

    pub fn source(&self) -> SystemReminderSource {
        self.source
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn rendered_body(&self) -> String {
        render_system_reminder(&self.body)
    }

    pub fn into_system_item(self) -> SystemItem {
        match self.source {
            SystemReminderSource::TaskInactivity => SystemItem::TaskReminder {
                source: self.source,
                body: self.rendered_body(),
                prompt_provenance: None,
            },
        }
    }
}

fn normalize_unwrapped_system_reminder_body(body: String) -> String {
    let trimmed = body.trim();
    if let Some(inner) = trimmed
        .strip_prefix(SYSTEM_REMINDER_OPEN)
        .and_then(|rest| rest.strip_suffix(SYSTEM_REMINDER_CLOSE))
    {
        return inner.trim_matches('\n').to_string();
    }
    body
}

fn render_system_reminder(body: &str) -> String {
    format!("{SYSTEM_REMINDER_OPEN}\n{body}\n{SYSTEM_REMINDER_CLOSE}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptRenderProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub config_revision: u64,
    pub source_digest: String,
    pub projection_digest: String,
    pub logical_name: String,
}

/// One agent-injected system item, tagged by origin.
///
/// Each variant carries the kind-specific raw data clients use for
/// typed rendering (`Notification.message`, `WorkerEvent.event`, file
/// path / identifier / etc.), plus a pre-rendered
/// `body` (where applicable) that is the exact `role:system` text the
/// LLM actually saw at commit time. `body` is denormalised so that
/// segment log replay reconstructs worker history byte-identical to
/// what was on the wire — even when prompt overrides (e.g. custom
/// `notify_wrapper` template) re-shape the live rendering on a later
/// resume.
///
/// New variants get added here as fresh injection kinds come online
/// (e.g. `TaskReminder`). The `kind` JSON tag is the snake_case form of
/// the variant name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SystemItem {
    /// Free-form notification sent in by an external caller via
    /// `Method::Notify`. `message` is the raw caller-supplied text;
    /// `body` is the wrapped LLM-context form (Worker renders it via
    /// `notify_wrapper` at commit time).
    Notification {
        message: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_provenance: Option<PromptRenderProvenance>,
    },

    /// Lifecycle event reported by a child Worker via `Method::WorkerEvent`.
    /// `event` is the typed payload (so the TUI can render per-child
    /// banners without re-parsing); `body` is the wrapped LLM-context
    /// form (same `notify_wrapper` path as `Notification`).
    WorkerEvent {
        event: WorkerEvent,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_provenance: Option<PromptRenderProvenance>,
    },

    /// `@<path>` file reference resolution. `body` is the rendered
    /// LLM-context text (`[File: <path>]\n…` for regular files,
    /// `[Dir: <path>]\n…` for directory listings, possibly with a
    /// truncation hint) so replay reconstructs worker history
    /// byte-identical to what was sent.
    FileAttachment { path: String, body: String },

    /// Explicit Agent Skill activation. `body` is the exact LLM-facing
    /// Skill text committed before any subsequent LLM request can use it.
    SkillActivation { name: String, body: String },

    /// Historical persisted Knowledge reference resolution. Knowledge is no
    /// longer active, so restored sessions ignore this item instead of replaying
    /// archived record text into model context.
    #[serde(rename = "knowledge")]
    LegacyKnowledgeIgnored { slug: String, body: String },

    /// Compatibility sink for pre-removal persisted `kind: "workflow"`
    /// system items. These entries are intentionally not replayed as
    /// authority-bearing context.
    #[serde(rename = "workflow")]
    LegacyIgnored { slug: String },

    /// Task-management inactivity reminder inserted before an LLM request.
    /// `source` is the policy that produced this durable reminder; `body` is
    /// the exact LLM-context text wrapped in a `<system-reminder>` block.
    TaskReminder {
        #[serde(default = "default_task_reminder_source")]
        source: SystemReminderSource,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_provenance: Option<PromptRenderProvenance>,
    },

    /// Synthetic note inserted after an interrupted turn before the next
    /// user input. `body` is the exact LLM-context text explaining that the
    /// previous turn was cut short.
    Interrupt {
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_provenance: Option<PromptRenderProvenance>,
    },
}

impl SystemItem {
    /// Free-text body the LLM sees inside its `role:system` message
    /// for this item. Returns the variant's stored `body` verbatim.
    pub fn history_text(&self) -> String {
        match self {
            SystemItem::Notification { body, .. } => body.clone(),
            SystemItem::WorkerEvent { body, .. } => body.clone(),
            SystemItem::FileAttachment { body, .. } => body.clone(),
            SystemItem::SkillActivation { body, .. } => body.clone(),
            SystemItem::LegacyKnowledgeIgnored { .. } => String::new(),
            SystemItem::LegacyIgnored { slug } => {
                format!("Ignored legacy procedure item: /{slug}")
            }
            SystemItem::TaskReminder { body, .. } => body.clone(),
            SystemItem::Interrupt { body, .. } => body.clone(),
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
            SystemItem::WorkerEvent { .. } => "worker_event",
            SystemItem::FileAttachment { .. } => "file_attachment",
            SystemItem::SkillActivation { .. } => "skill_activation",
            SystemItem::LegacyKnowledgeIgnored { .. } => "legacy_knowledge_ignored",
            SystemItem::LegacyIgnored { .. } => "legacy_ignored",
            SystemItem::TaskReminder { .. } => "task_reminder",
            SystemItem::Interrupt { .. } => "interrupt",
        }
    }
}

/// Render a `WorkerEvent` as the one-line notification text the agent
/// sees. Centralised here (rather than at the controller's render
/// site) so persistence and broadcast share the same rendering.
pub fn render_worker_event(event: &WorkerEvent) -> String {
    match event {
        WorkerEvent::TurnEnded { worker_name } => format!("worker `{worker_name}` finished a turn"),
        WorkerEvent::Errored {
            worker_name,
            message,
        } => {
            format!("worker `{worker_name}` errored: {message}")
        }
        WorkerEvent::ShutDown { worker_name } => format!("worker `{worker_name}` shut down"),
        WorkerEvent::ScopeSubDelegated {
            parent_worker,
            sub_worker,
            ..
        } => {
            format!("worker `{parent_worker}` sub-delegated scope to `{sub_worker}`")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_prompt_rendered_items_default_missing_provenance() {
        let notification: SystemItem = serde_json::from_str(
            r#"{"kind":"notification","message":"legacy","body":"legacy body"}"#,
        )
        .unwrap();
        let interrupt: SystemItem =
            serde_json::from_str(r#"{"kind":"interrupt","body":"legacy interrupt"}"#).unwrap();
        assert!(matches!(
            notification,
            SystemItem::Notification {
                prompt_provenance: None,
                ..
            }
        ));
        assert!(matches!(
            interrupt,
            SystemItem::Interrupt {
                prompt_provenance: None,
                ..
            }
        ));
    }

    #[test]
    fn notification_history_text_returns_stored_body() {
        let item = SystemItem::Notification {
            message: "child done".into(),
            body: "[Notification]\nchild done\n\n(non-blocking hint…)".into(),
            prompt_provenance: None,
        };
        assert_eq!(
            item.history_text(),
            "[Notification]\nchild done\n\n(non-blocking hint…)"
        );
    }

    #[test]
    fn worker_event_history_text_returns_stored_body() {
        let item = SystemItem::WorkerEvent {
            event: WorkerEvent::TurnEnded {
                worker_name: "child".into(),
            },
            body: "[Notification]\npod `child` finished a turn\n\n(non-blocking hint…)".into(),
            prompt_provenance: None,
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
    fn system_reminder_renders_body_once() {
        let reminder = SystemReminder::task_inactivity("remember tasks");
        assert_eq!(
            reminder.rendered_body(),
            "<system-reminder>\nremember tasks\n</system-reminder>"
        );

        let already_wrapped = SystemReminder::task_inactivity(
            "<system-reminder>\nremember tasks\n</system-reminder>",
        );
        assert_eq!(already_wrapped.body(), "remember tasks");
        assert_eq!(
            already_wrapped.rendered_body(),
            "<system-reminder>\nremember tasks\n</system-reminder>"
        );
    }

    #[test]
    fn system_reminder_source_is_retained_in_system_item() {
        let item = SystemReminder::task_inactivity("remember tasks").into_system_item();
        match item {
            SystemItem::TaskReminder { source, body, .. } => {
                assert_eq!(source, SystemReminderSource::TaskInactivity);
                assert_eq!(
                    body,
                    "<system-reminder>\nremember tasks\n</system-reminder>"
                );
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn task_reminder_deserialization_defaults_legacy_source() {
        let parsed: SystemItem = serde_json::from_str(
            r#"{"kind":"task_reminder","body":"<system-reminder>\nbody\n</system-reminder>"}"#,
        )
        .unwrap();
        match parsed {
            SystemItem::TaskReminder { source, .. } => {
                assert_eq!(source, SystemReminderSource::TaskInactivity);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn legacy_procedure_system_item_is_ignored_on_replay() {
        let parsed: SystemItem =
            serde_json::from_str(r#"{"kind":"workflow","slug":"old-flow","body":"legacy body"}"#)
                .unwrap();
        assert_eq!(
            parsed.history_text(),
            "Ignored legacy procedure item: /old-flow"
        );
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
    fn round_trip_worker_event() {
        let item = SystemItem::WorkerEvent {
            event: WorkerEvent::TurnEnded {
                worker_name: "child".into(),
            },
            body: "[Notification] worker `child` finished a turn".into(),
            prompt_provenance: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        let parsed: SystemItem = serde_json::from_str(&json).unwrap();
        match parsed {
            SystemItem::WorkerEvent {
                event: WorkerEvent::TurnEnded { worker_name },
                body,
                ..
            } => {
                assert_eq!(worker_name, "child");
                assert!(body.contains("`child`"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
