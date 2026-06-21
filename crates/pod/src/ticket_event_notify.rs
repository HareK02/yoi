use std::sync::Arc;

use async_trait::async_trait;
use minijinja::Value as TemplateValue;
use serde_json::Value;
use std::collections::BTreeMap;
use ticket::{LocalTicketBackend, TicketBackend, TicketIdOrSlug};
use tracing::{debug, warn};

use crate::discovery::{PodDiscovery, WeakNotifyDelivery};
use crate::hook::{Hook, HookPostToolAction, PostToolCall, ToolResultSummary};
use crate::prompt::catalog::{PodPrompt, PromptCatalog};
use pod_store::PodMetadataStore;

const MAX_TITLE_CHARS: usize = 96;
const MAX_SUMMARY_CHARS: usize = 160;
const MAX_EVENT_KIND_CHARS: usize = 80;
const MAX_MESSAGE_CHARS: usize = 768;

#[derive(Clone)]
pub(crate) struct TicketEventCompanionNotifyHook<
    St: PodMetadataStore + Clone + Send + Sync + 'static,
> {
    backend: Arc<LocalTicketBackend>,
    discovery: PodDiscovery<St>,
    companion_pod_name: String,
}

impl<St: PodMetadataStore + Clone + Send + Sync + 'static> TicketEventCompanionNotifyHook<St> {
    pub(crate) fn new(
        backend: LocalTicketBackend,
        discovery: PodDiscovery<St>,
        companion_pod_name: impl Into<String>,
    ) -> Self {
        Self {
            backend: Arc::new(backend),
            discovery,
            companion_pod_name: companion_pod_name.into(),
        }
    }
}

#[async_trait]
impl<St: PodMetadataStore + Clone + Send + Sync + 'static> Hook<PostToolCall>
    for TicketEventCompanionNotifyHook<St>
{
    async fn call(&self, summary: &ToolResultSummary) -> HookPostToolAction {
        let Some(notice) = build_ticket_event_notice(&self.backend, summary) else {
            return HookPostToolAction::Continue;
        };
        match self
            .discovery
            .ensure_existing_peer(&self.companion_pod_name)
        {
            Ok(Some(_)) => {
                debug!(
                    ticket = %notice.ticket_id,
                    event_kind = %notice.event_kind,
                    companion = %self.companion_pod_name,
                    "ensured Companion peer relationship before Ticket event notification"
                );
            }
            Ok(None) => {
                debug!(
                    ticket = %notice.ticket_id,
                    event_kind = %notice.event_kind,
                    companion = %self.companion_pod_name,
                    "skipping Companion peer registration because Companion metadata is missing"
                );
            }
            Err(error) => {
                warn!(
                    ticket = %notice.ticket_id,
                    event_kind = %notice.event_kind,
                    companion = %self.companion_pod_name,
                    error = %error,
                    "failed to ensure Companion peer relationship before Ticket event notification"
                );
            }
        }
        let delivery = self
            .discovery
            .send_weak_notify_to_live_peer(&self.companion_pod_name, notice.message)
            .await;
        match delivery {
            WeakNotifyDelivery::Delivered => {
                debug!(
                    ticket = %notice.ticket_id,
                    event_kind = %notice.event_kind,
                    companion = %self.companion_pod_name,
                    "delivered weak Ticket event notification to Companion peer"
                );
            }
            skipped => {
                warn!(
                    ticket = %notice.ticket_id,
                    event_kind = %notice.event_kind,
                    companion = %self.companion_pod_name,
                    delivery = %skipped,
                    "skipped weak Ticket event notification to Companion peer"
                );
            }
        }
        HookPostToolAction::Continue
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TicketEventNotice {
    ticket_id: String,
    event_kind: String,
    message: String,
}

fn build_ticket_event_notice(
    backend: &LocalTicketBackend,
    summary: &ToolResultSummary,
) -> Option<TicketEventNotice> {
    if summary.is_error {
        return None;
    }
    let output = &summary.output;
    let content = output.content.as_deref()?;
    let content: Value = serde_json::from_str(content).ok()?;
    if !content.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }

    let event_kind = explicit_ticket_event_kind(summary.tool_name.as_str(), &content)?;
    let ticket_query = content.get("ticket").and_then(Value::as_str)?;
    let ticket = backend
        .show(TicketIdOrSlug::Query(ticket_query.to_string()))
        .ok()?;

    let event_kind = sanitize_one_line(&event_kind, MAX_EVENT_KIND_CHARS);
    let ticket_id = ticket.meta.id.as_str();
    let title = sanitize_one_line(&ticket.meta.title, MAX_TITLE_CHARS);
    let state = ticket.meta.workflow_state.as_str();
    let output_summary = sanitize_one_line(&output.summary, MAX_SUMMARY_CHARS);
    let ref_path = event_ref_path(ticket_id, summary.tool_name.as_str());
    let message = render_ticket_event_notice_message(TicketEventNoticeValues {
        ticket_id,
        title: &title,
        state,
        event_kind: &event_kind,
        summary: &output_summary,
        ref_path: &ref_path,
    })?;

    Some(TicketEventNotice {
        ticket_id: ticket_id.to_string(),
        event_kind,
        message: bound_chars(&message, MAX_MESSAGE_CHARS),
    })
}

struct TicketEventNoticeValues<'a> {
    ticket_id: &'a str,
    title: &'a str,
    state: &'a str,
    event_kind: &'a str,
    summary: &'a str,
    ref_path: &'a str,
}

fn render_ticket_event_notice_message(values: TicketEventNoticeValues<'_>) -> Option<String> {
    PromptCatalog::builtins_only()
        .ok()?
        .render(PodPrompt::TicketEventCompanionNotice, values.to_template())
        .ok()
}

impl TicketEventNoticeValues<'_> {
    fn to_template(&self) -> TemplateValue {
        let mut values: BTreeMap<&'static str, TemplateValue> = BTreeMap::new();
        values.insert("ticket_id", TemplateValue::from(self.ticket_id));
        values.insert("title", TemplateValue::from(self.title));
        values.insert("state", TemplateValue::from(self.state));
        values.insert("event_kind", TemplateValue::from(self.event_kind));
        values.insert("summary", TemplateValue::from(self.summary));
        values.insert("ref_path", TemplateValue::from(self.ref_path));
        TemplateValue::from(values)
    }
}

fn explicit_ticket_event_kind(tool_name: &str, content: &Value) -> Option<String> {
    match tool_name {
        "TicketComment" => content
            .get("event")
            .and_then(Value::as_str)
            .map(|event| format!("comment/{event}")),
        "TicketReview" => content
            .get("review")
            .and_then(Value::as_str)
            .map(|review| format!("review/{review}")),
        "TicketWorkflowState" => {
            let from = content.get("from").and_then(Value::as_str).unwrap_or("?");
            let to = content.get("to").and_then(Value::as_str).unwrap_or("?");
            Some(format!("state/{from}->{to}"))
        }
        "TicketIntakeReady" => Some("state/planning->ready".to_string()),
        "TicketClose" => Some("close/resolution".to_string()),
        _ => None,
    }
}

fn event_ref_path(ticket_id: &str, tool_name: &str) -> String {
    let leaf = match tool_name {
        "TicketClose" => "resolution.md",
        "TicketIntakeReady" | "TicketWorkflowState" => "item.md",
        _ => "thread.md",
    };
    format!(".yoi/tickets/{ticket_id}/{leaf}")
}

fn sanitize_one_line(input: &str, limit: usize) -> String {
    let collapsed = input.split_whitespace().collect::<Vec<_>>().join(" ");
    bound_chars(&collapsed, limit)
}

fn bound_chars(input: &str, limit: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in input.chars().filter(|ch| !ch.is_control()).enumerate() {
        if idx >= limit {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

pub(crate) fn companion_pod_name_for_workspace(workspace_root: &std::path::Path) -> Option<String> {
    workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PodStatus;
    use crate::runtime::dir::RuntimeDir;
    use crate::spawn::registry::SpawnedPodRegistry;
    use llm_worker::tool::ToolOutput;
    use pod_store::FsPodStore;
    use pod_store::PodMetadata;
    use protocol::stream::{JsonLineReader, JsonLineWriter};
    use protocol::{Event, Method};
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::tempdir;
    use ticket::NewTicket;
    use tokio::net::UnixListener;

    fn create_backend_with_ticket(title: &str) -> (tempfile::TempDir, LocalTicketBackend, String) {
        let dir = tempdir().expect("tempdir");
        let backend = LocalTicketBackend::new(dir.path().to_path_buf());
        let mut input = NewTicket::new(title);
        input.body = ticket::MarkdownText::new("body");
        let ticket = backend.create(input).expect("create ticket");
        (dir, backend, ticket.id)
    }

    fn tool_summary(tool_name: &str, output: ToolOutput) -> ToolResultSummary {
        ToolResultSummary {
            call_id: "test-call".to_string(),
            tool_name: tool_name.to_string(),
            output,
            is_error: false,
        }
    }

    #[test]
    fn builds_bounded_event_scoped_notice_for_ticket_state_change() {
        let (_dir, backend, ticket_id) = create_backend_with_ticket(
            "A very long title that should be bounded but still identify the ticket precisely enough for Companion",
        );
        let output = ToolOutput {
            summary: "Changed ticket state from queued to inprogress with a deliberately long summary that should be bounded before entering the weak notification payload and should not contain large logs".into(),
            content: Some(
                json!({
                    "ok": true,
                    "ticket": ticket_id,
                    "from": "queued",
                    "to": "inprogress",
                })
                .to_string(),
            ),
        };

        let notice =
            build_ticket_event_notice(&backend, &tool_summary("TicketWorkflowState", output))
                .expect("notice");

        assert_eq!(notice.ticket_id, ticket_id);
        assert_eq!(notice.event_kind, "state/queued->inprogress");
        assert!(notice.message.contains("auto_run=false"));
        assert!(notice.message.contains("event: state/queued->inprogress"));
        assert!(notice.message.contains("ref: .yoi/tickets/"));
        assert!(notice.message.chars().count() <= MAX_MESSAGE_CHARS + 1);

        let expected = PromptCatalog::builtins_only()
            .expect("load prompt catalog")
            .render(
                PodPrompt::TicketEventCompanionNotice,
                TicketEventNoticeValues {
                    ticket_id: &notice.ticket_id,
                    title: &sanitize_one_line(
                        "A very long title that should be bounded but still identify the ticket precisely enough for Companion",
                        MAX_TITLE_CHARS,
                    ),
                    state: "planning",
                    event_kind: "state/queued->inprogress",
                    summary: &sanitize_one_line(
                        "Changed ticket state from queued to inprogress with a deliberately long summary that should be bounded before entering the weak notification payload and should not contain large logs",
                        MAX_SUMMARY_CHARS,
                    ),
                    ref_path: &format!(".yoi/tickets/{}/item.md", ticket_id),
                }
                .to_template(),
            )
            .expect("render prompt resource");
        assert_eq!(notice.message, bound_chars(&expected, MAX_MESSAGE_CHARS));
    }

    #[test]
    fn ignores_passive_or_non_event_ticket_tools() {
        let (_dir, backend, ticket_id) = create_backend_with_ticket("Passive list test");
        let output = ToolOutput {
            summary: "Listed tickets".into(),
            content: Some(json!({"ok": true, "ticket": ticket_id}).to_string()),
        };

        assert!(build_ticket_event_notice(&backend, &tool_summary("TicketList", output)).is_none());
    }

    #[test]
    fn notice_does_not_include_tool_content_body_or_error_details() {
        let (_dir, backend, ticket_id) = create_backend_with_ticket("Safe payload");
        let output = ToolOutput {
            summary: "Appended implementation_report to ticket".into(),
            content: Some(
                json!({
                    "ok": true,
                    "ticket": ticket_id,
                    "event": "implementation_report",
                    "body": "SECRET_TOKEN provider stack trace long diagnostic should not be copied",
                    "error": "provider error details should not be copied"
                })
                .to_string(),
            ),
        };

        let notice = build_ticket_event_notice(&backend, &tool_summary("TicketComment", output))
            .expect("notice");

        assert!(
            notice
                .message
                .contains("event: comment/implementation_report")
        );
        assert!(!notice.message.contains("SECRET_TOKEN"));
        assert!(!notice.message.contains("provider error details"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ticket_event_hook_ensures_peer_and_delivers_weak_companion_notification() {
        let root = tempdir().expect("tempdir");
        let runtime_base = root.path().join("runtime");
        let store_dir = root.path().join("store");
        std::fs::create_dir_all(runtime_base.join("companion")).unwrap();
        let store = FsPodStore::new(&store_dir).unwrap();
        store
            .write(&PodMetadata {
                pod_name: "orchestrator".into(),
                active: None,
                workspace_root: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: Vec::new(),
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&PodMetadata {
                pod_name: "companion".into(),
                active: None,
                workspace_root: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: Vec::new(),
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        let (_ticket_dir, backend, ticket_id) = create_backend_with_ticket("Companion event hook");
        let runtime_dir = Arc::new(
            RuntimeDir::create(&runtime_base, "orchestrator")
                .await
                .unwrap(),
        );
        let store_for_assert = store.clone();
        let hook = TicketEventCompanionNotifyHook::new(
            backend,
            PodDiscovery::new(
                store,
                "orchestrator".into(),
                runtime_base.clone(),
                root.path().to_path_buf(),
                SpawnedPodRegistry::new(runtime_dir),
            ),
            "companion",
        );

        let socket = runtime_base.join("companion").join("sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let companion = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut writer = JsonLineWriter::new(stream);
            writer
                .write(&Event::Snapshot {
                    entries: Vec::new(),
                    greeting: protocol::Greeting {
                        pod_name: "companion".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 0,
                        context_tokens: 0,
                    },
                    status: PodStatus::Idle,
                    in_flight: Default::default(),
                })
                .await
                .unwrap();

            let (stream, _) = listener.accept().await.unwrap();
            let (reader_half, writer_half) = stream.into_split();
            let mut reader = JsonLineReader::new(reader_half);
            let mut writer = JsonLineWriter::new(writer_half);
            writer
                .write(&Event::Snapshot {
                    entries: Vec::new(),
                    greeting: protocol::Greeting {
                        pod_name: "companion".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 0,
                        context_tokens: 0,
                    },
                    status: PodStatus::Idle,
                    in_flight: Default::default(),
                })
                .await
                .unwrap();
            let method = reader.next::<Method>().await.unwrap().unwrap();
            if let Method::Notify { message, auto_run } = method {
                assert!(!auto_run);
                tx.send(message).await.unwrap();
            } else {
                panic!("expected Notify, got {method:?}");
            }
        });

        let output = ToolOutput {
            summary: "Changed ticket state from queued to inprogress".into(),
            content: Some(
                json!({
                    "ok": true,
                    "ticket": ticket_id,
                    "from": "queued",
                    "to": "inprogress",
                })
                .to_string(),
            ),
        };
        let action = hook
            .call(&tool_summary("TicketWorkflowState", output))
            .await;
        assert_eq!(action, HookPostToolAction::Continue);
        let message = rx.recv().await.unwrap();
        assert!(message.contains("event: state/queued->inprogress"));
        assert!(message.contains("title: Companion event hook"));
        let orchestrator = store_for_assert
            .read_by_name("orchestrator")
            .unwrap()
            .unwrap();
        assert_eq!(orchestrator.peers.len(), 1);
        assert_eq!(orchestrator.peers[0].pod_name, "companion");
        let companion_metadata = store_for_assert.read_by_name("companion").unwrap().unwrap();
        assert_eq!(companion_metadata.peers.len(), 1);
        assert_eq!(companion_metadata.peers[0].pod_name, "orchestrator");
        companion.await.unwrap();
    }
}
