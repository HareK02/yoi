//! Typed records of system input a Worker presents to the model.
//!
//! Notifications, child events, reference resolution, and reminders are Worker-added
//! input, not model output. [`SystemItem`] retains the display kind and model-visible
//! body, representing the same input in history and live events. Stored bodies allow
//! old input to be reproduced even if prompt rendering rules later change.
//!
//! Input affecting the model is committed via [`crate::LogEntry::AnnotatedSystemItem`]
//! before use. Request-only transient insertion would leave later turns unable to
//! explain decisions from stored history. The model projection uses only the body;
//! clients can render using type information without parsing body prefixes.

use agen::llm_client::types::Item;
use protocol::WorkerEvent;
use serde::{Deserialize, Serialize};

/// Source policy that produced a durable system reminder input.
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
    /// Build a task-inactivity reminder from its plain system-message body.
    pub fn task_inactivity(body: impl Into<String>) -> Self {
        Self::new(SystemReminderSource::TaskInactivity, body)
    }

    /// Build a reminder whose body is committed verbatim as a system message.
    pub fn new(source: SystemReminderSource, body: impl Into<String>) -> Self {
        Self {
            source,
            body: body.into(),
        }
    }

    pub fn into_system_item(self) -> SystemItem {
        match self.source {
            SystemReminderSource::TaskInactivity => SystemItem::TaskReminder {
                source: self.source,
                body: self.body,
                prompt_provenance: None,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptRenderProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
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
    /// Generic System body; private metadata and public presentation are supplied
    /// independently by the producer in the same annotated history record.
    Input { body: String },
    /// One-time derived Journal reference context. The bounded Host snapshot is
    /// retained as JSON independently of its rendered body and prompt projection.
    JournalStartup {
        body: String,
        snapshot: serde_json::Value,
        snapshot_sha256: String,
        prompt_provenance: PromptRenderProvenance,
    },
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

    /// Result of an explicitly selected chat Feature invocation. The typed
    /// result is retained for clients; `body` is the exact bounded context made
    /// visible to the Worker model.
    FeatureInvocationResult {
        result: protocol::FeatureInvocationResult,
        body: String,
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
    /// the exact plain system-message text committed to LLM context.
    TaskReminder {
        #[serde(default = "default_task_reminder_source")]
        source: SystemReminderSource,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_provenance: Option<PromptRenderProvenance>,
    },

    /// Restore-boundary refresh of the current resident Memory projection.
    /// The exact rendered body is retained so replay is byte-identical; a
    /// refresh with no ready summary explicitly supersedes older resident
    /// context without deleting or rewriting history.
    ResidentSummaryRefresh {
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_provenance: Option<PromptRenderProvenance>,
    },

    /// A Host-fetched change to user-managed Subject behavior. Each changed
    /// body is appended rather than rewriting prior prompt/session content.
    SubjectBehaviorRefresh {
        body: String,
        /// Exact user-managed behavior acknowledged after durable commit. Old
        /// logs omit this value; an empty value is not an acknowledgement of a
        /// nonempty behavior and does not replace their stored rendered body.
        #[serde(default)]
        behavior_md: String,
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

/// Allowlisted presentation data shared by live events and retained snapshots.
/// Private Feature payloads and render provenance never cross this boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicSystemItem {
    pub kind: String,
    pub body: String,
    pub presentation: PublicSystemPresentation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublicSystemPresentation {
    Notification { message: String },
    WorkerEvent { event: WorkerEvent },
    Context,
    TaskReminder,
    Ignored,
}

impl SystemItem {
    pub fn public_item(&self) -> PublicSystemItem {
        let presentation = match self {
            Self::Notification { message, .. } => PublicSystemPresentation::Notification {
                message: message.clone(),
            },
            Self::WorkerEvent { event, .. } => PublicSystemPresentation::WorkerEvent {
                event: event.clone(),
            },
            Self::TaskReminder { .. } => PublicSystemPresentation::TaskReminder,
            Self::Input { .. }
            | Self::LegacyKnowledgeIgnored { .. }
            | Self::LegacyIgnored { .. } => PublicSystemPresentation::Ignored,
            Self::JournalStartup { .. }
            | Self::ResidentSummaryRefresh { .. }
            | Self::SubjectBehaviorRefresh { .. }
            | Self::SkillActivation { .. }
            | Self::FeatureInvocationResult { .. }
            | Self::FileAttachment { .. }
            | Self::Interrupt { .. } => PublicSystemPresentation::Context,
        };
        PublicSystemItem {
            kind: self.kind_label().to_owned(),
            body: if matches!(presentation, PublicSystemPresentation::Ignored) {
                String::new()
            } else {
                self.history_text()
            },
            presentation,
        }
    }

    /// Free-text body the LLM sees inside its `role:system` message
    /// for this item. Returns the variant's stored `body` verbatim.
    pub fn history_text(&self) -> String {
        match self {
            SystemItem::Input { body } => body.clone(),
            SystemItem::JournalStartup { body, .. } => body.clone(),
            SystemItem::Notification { body, .. } => body.clone(),
            SystemItem::WorkerEvent { body, .. } => body.clone(),
            SystemItem::FeatureInvocationResult { body, .. } => body.clone(),
            SystemItem::FileAttachment { body, .. } => body.clone(),
            SystemItem::SkillActivation { body, .. } => body.clone(),
            SystemItem::LegacyKnowledgeIgnored { .. } => String::new(),
            SystemItem::LegacyIgnored { slug } => {
                format!("Ignored legacy procedure item: /{slug}")
            }
            SystemItem::TaskReminder { body, .. } => body.clone(),
            SystemItem::ResidentSummaryRefresh { body, .. } => body.clone(),
            SystemItem::SubjectBehaviorRefresh { body, .. } => body.clone(),
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
            SystemItem::Input { .. } => "input",
            SystemItem::JournalStartup { .. } => "journal_startup",
            SystemItem::Notification { .. } => "notification",
            SystemItem::WorkerEvent { .. } => "worker_event",
            SystemItem::FeatureInvocationResult { .. } => "feature_invocation_result",
            SystemItem::FileAttachment { .. } => "file_attachment",
            SystemItem::SkillActivation { .. } => "skill_activation",
            SystemItem::LegacyKnowledgeIgnored { .. } => "legacy_knowledge_ignored",
            SystemItem::LegacyIgnored { .. } => "legacy_ignored",
            SystemItem::TaskReminder { .. } => "task_reminder",
            SystemItem::ResidentSummaryRefresh { .. } => "resident_summary_refresh",
            SystemItem::SubjectBehaviorRefresh { .. } => "subject_behavior_refresh",
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
    fn subject_behavior_refresh_round_trips_with_exact_body() {
        let item = SystemItem::SubjectBehaviorRefresh {
            body: "current Subject context".to_string(),
            behavior_md: "exact behavior".to_string(),
            prompt_provenance: None,
        };
        let raw = serde_json::to_string(&item).unwrap();
        let parsed: SystemItem = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.history_text(), "current Subject context");
        assert!(
            matches!(parsed, SystemItem::SubjectBehaviorRefresh { behavior_md, .. } if behavior_md == "exact behavior")
        );
    }

    #[test]
    fn public_system_input_decoder_rejects_private_fields_and_unknown_presentations() {
        let valid = serde_json::json!({
            "kind": "new-feature-label",
            "body": "explicit public content",
            "presentation": { "kind": "context" },
        });
        let public: PublicSystemItem = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(public.body, "explicit public content");
        for private_field in [
            "payload",
            "prompt_provenance",
            "policy",
            "snapshot",
            "behavior_md",
        ] {
            let mut malformed = valid.clone();
            malformed[private_field] = serde_json::json!({ "private": true });
            assert!(serde_json::from_value::<PublicSystemItem>(malformed).is_err());
        }
        let mut unknown = valid;
        unknown["presentation"] = serde_json::json!({ "kind": "execute_feature_payload" });
        assert!(serde_json::from_value::<PublicSystemItem>(unknown).is_err());
    }

    #[test]
    fn frozen_subject_behavior_log_reads_old_counter_without_republishing_it() {
        let old = r#"{"kind":"subject_behavior_refresh","body":"exact archived body","behavior_revision":7}"#;
        let item: SystemItem = serde_json::from_str(old).unwrap();
        assert_eq!(item.history_text(), "exact archived body");
        assert_eq!(
            serde_json::to_value(item).unwrap(),
            serde_json::json!({
                "kind": "subject_behavior_refresh",
                "body": "exact archived body",
                "behavior_md": ""
            })
        );
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
    fn system_reminder_preserves_plain_body() {
        let item = SystemReminder::task_inactivity("remember tasks").into_system_item();
        assert_eq!(item.history_text(), "remember tasks");
    }

    #[test]
    fn system_reminder_source_is_retained_in_system_item() {
        let item = SystemReminder::task_inactivity("remember tasks").into_system_item();
        match item {
            SystemItem::TaskReminder { source, body, .. } => {
                assert_eq!(source, SystemReminderSource::TaskInactivity);
                assert_eq!(body, "remember tasks");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn task_reminder_deserialization_defaults_legacy_source() {
        let parsed: SystemItem =
            serde_json::from_str(r#"{"kind":"task_reminder","body":"legacy body"}"#).unwrap();
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
    fn invocation_result_retains_typed_outcome_and_exact_context() {
        let result = protocol::FeatureInvocationResult {
            invocation_id: "stable-invoke".into(),
            identity: protocol::FeatureInvocationIdentity("builtin:test/prepare".into()),
            status: protocol::FeatureInvocationStatus::OutcomeUnknown,
            message: "operation may have committed".into(),
            context: None,
        };
        let body = "Outcome unknown; do not retry automatically";
        let item = SystemItem::FeatureInvocationResult {
            result: result.clone(),
            body: body.into(),
        };
        let restored: SystemItem =
            serde_json::from_str(&serde_json::to_string(&item).unwrap()).unwrap();
        assert_eq!(restored.history_text(), body);
        assert_eq!(restored.kind_label(), "feature_invocation_result");
        assert!(
            matches!(restored, SystemItem::FeatureInvocationResult { result: saved, .. } if saved == result)
        );
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
