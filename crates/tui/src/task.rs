//! In-TUI mirror of the session-lifetime task store.
//!
//! This deliberately does NOT depend on the Worker TaskStore. The TUI is a
//! presentation layer; pulling in `worker` would drag along the runtime
//! feature surface. Instead we mirror the small subset we
//! need:
//!
//! - `TaskEntry` / `TaskStatus`: shaped to round-trip with Worker Task JSON
//!   serialization (`#[serde(rename_all = "lowercase")]` on the status,
//!   matching field names on the entry).
//! - Just enough state machine to apply `TaskCreate` / `TaskUpdate`
//!   tool-call arguments and the `[Session TaskStore snapshot]` system
//!   message that compaction emits.
//!
//! The snapshot text format is owned by the Worker Task feature. The TUI keeps
//! local compatibility fixtures for the `[Session TaskStore snapshot]` system
//! message shape emitted during compaction and restored on resume.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Inprogress,
    Completed,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TaskEntry {
    pub taskid: u64,
    pub status: TaskStatus,
    pub subject: String,
    pub description: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TaskCounts {
    pub pending: usize,
    pub inprogress: usize,
    pub completed: usize,
    pub deleted: usize,
}

impl TaskCounts {
    pub fn total(&self) -> usize {
        self.pending + self.inprogress + self.completed + self.deleted
    }

    pub fn active(&self) -> usize {
        self.pending + self.inprogress
    }
}

#[derive(Debug, Default, Clone)]
pub struct TaskStore {
    next_taskid: u64,
    tasks: Vec<TaskEntry>,
}

impl TaskStore {
    pub fn new() -> Self {
        Self {
            next_taskid: 1,
            tasks: Vec::new(),
        }
    }

    pub fn tasks(&self) -> &[TaskEntry] {
        &self.tasks
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn counts(&self) -> TaskCounts {
        let mut c = TaskCounts::default();
        for t in &self.tasks {
            match t.status {
                TaskStatus::Pending => c.pending += 1,
                TaskStatus::Inprogress => c.inprogress += 1,
                TaskStatus::Completed => c.completed += 1,
                TaskStatus::Deleted => c.deleted += 1,
            }
        }
        c
    }

    /// Apply a completed `TaskCreate` / `TaskUpdate` tool_call. Other
    /// tool names and unparseable JSON are silent no-ops, matching the
    /// resilience of the Worker TaskStore history replay.
    pub fn apply_tool_call(&mut self, name: &str, arguments: &str) {
        match name {
            "TaskCreate" => {
                if let Ok(p) = serde_json::from_str::<TaskCreateParams>(arguments) {
                    self.tasks.push(TaskEntry {
                        taskid: self.next_taskid,
                        status: TaskStatus::Pending,
                        subject: p.subject,
                        description: p.description,
                    });
                    self.next_taskid = self.next_taskid.saturating_add(1);
                }
            }
            "TaskUpdate" => {
                if let Ok(p) = serde_json::from_str::<TaskUpdateParams>(arguments)
                    && let Some(t) = self.tasks.iter_mut().find(|t| t.taskid == p.taskid)
                {
                    if let Some(s) = p.status {
                        t.status = s;
                    }
                    if let Some(s) = p.subject {
                        t.subject = s;
                    }
                    if let Some(d) = p.description {
                        t.description = d;
                    }
                }
            }
            _ => {}
        }
    }

    /// Replace all state from a `[Session TaskStore snapshot]` system
    /// message. No-op if the text doesn't carry one.
    pub fn apply_system_message_text(&mut self, text: &str) {
        if let Some(tasks) = parse_snapshot_text(text) {
            self.replace_with(tasks);
        }
    }

    fn replace_with(&mut self, tasks: Vec<TaskEntry>) {
        self.next_taskid = tasks
            .iter()
            .map(|t| t.taskid)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        self.tasks = tasks;
    }
}

#[derive(Debug, Deserialize)]
struct TaskCreateParams {
    subject: String,
    description: String,
}

#[derive(Debug, Deserialize)]
struct TaskUpdateParams {
    taskid: u64,
    #[serde(default)]
    status: Option<TaskStatus>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TaskSnapshot {
    tasks: Vec<TaskEntry>,
}

fn parse_snapshot_text(text: &str) -> Option<Vec<TaskEntry>> {
    if !text.starts_with("[Session TaskStore snapshot]") {
        return None;
    }
    let start_marker = "```json\n";
    let end_marker = "\n```";
    let start = text.find(start_marker)? + start_marker.len();
    let rest = &text[start..];
    let end = rest.find(end_marker)?;
    let snapshot: TaskSnapshot = serde_json::from_str(&rest[..end]).ok()?;
    Some(snapshot.tasks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_create_assigns_sequential_ids() {
        let mut s = TaskStore::new();
        s.apply_tool_call("TaskCreate", r#"{"subject":"a","description":"A"}"#);
        s.apply_tool_call("TaskCreate", r#"{"subject":"b","description":"B"}"#);
        let tasks = s.tasks();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].taskid, 1);
        assert_eq!(tasks[0].subject, "a");
        assert_eq!(tasks[0].status, TaskStatus::Pending);
        assert_eq!(tasks[1].taskid, 2);
    }

    #[test]
    fn task_update_changes_status_and_fields() {
        let mut s = TaskStore::new();
        s.apply_tool_call("TaskCreate", r#"{"subject":"a","description":"A"}"#);
        s.apply_tool_call(
            "TaskUpdate",
            r#"{"taskid":1,"status":"inprogress","subject":"a-renamed"}"#,
        );
        let t = &s.tasks()[0];
        assert_eq!(t.status, TaskStatus::Inprogress);
        assert_eq!(t.subject, "a-renamed");
        assert_eq!(t.description, "A");
    }

    #[test]
    fn malformed_arguments_are_silently_ignored() {
        let mut s = TaskStore::new();
        s.apply_tool_call("TaskCreate", r#"{"subject":1}"#);
        s.apply_tool_call("TaskCreate", "not json");
        s.apply_tool_call("Unknown", r#"{"subject":"x","description":"y"}"#);
        s.apply_tool_call("TaskUpdate", r#"{"taskid":99,"status":"deleted"}"#);
        assert!(s.tasks().is_empty());
    }

    #[test]
    fn counts_classifies_each_status() {
        let mut s = TaskStore::new();
        s.apply_tool_call("TaskCreate", r#"{"subject":"a","description":""}"#);
        s.apply_tool_call("TaskCreate", r#"{"subject":"b","description":""}"#);
        s.apply_tool_call("TaskCreate", r#"{"subject":"c","description":""}"#);
        s.apply_tool_call("TaskUpdate", r#"{"taskid":1,"status":"inprogress"}"#);
        s.apply_tool_call("TaskUpdate", r#"{"taskid":2,"status":"completed"}"#);
        let c = s.counts();
        assert_eq!(c.pending, 1);
        assert_eq!(c.inprogress, 1);
        assert_eq!(c.completed, 1);
        assert_eq!(c.deleted, 0);
        assert_eq!(c.total(), 3);
        assert_eq!(c.active(), 2);
    }

    /// Snapshot text matches the wrapping `Worker::try_pre_run_compact` and the
    /// Worker Task feature snapshot fixture shape: header line, blank, overview
    /// line, blank, fenced JSON, trailing prose.
    fn wrap_snapshot(json_body: &str, overview: &str) -> String {
        format!(
            "[Session TaskStore snapshot]\n\n{overview}\n\n```json\n{json_body}\n```\n\n\
             This is the complete session task list preserved across compaction. \
             The following TaskList tool result presents the same state through the tool lane."
        )
    }

    #[test]
    fn snapshot_text_replaces_state_and_advances_next_id() {
        let body = r#"{
  "tasks": [
    {
      "taskid": 5,
      "status": "completed",
      "subject": "first",
      "description": "first desc"
    },
    {
      "taskid": 7,
      "status": "pending",
      "subject": "second",
      "description": "second desc"
    }
  ]
}"#;
        let text = wrap_snapshot(
            body,
            "TaskStore: 2 task(s) (pending: 1, inprogress: 0, completed: 1, deleted: 0)",
        );
        let mut s = TaskStore::new();
        s.apply_tool_call("TaskCreate", r#"{"subject":"stale","description":""}"#);
        s.apply_system_message_text(&text);
        let tasks = s.tasks();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].taskid, 5);
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(tasks[1].taskid, 7);
        // Subsequent TaskCreate must continue beyond the highest taskid
        // observed in the snapshot.
        s.apply_tool_call("TaskCreate", r#"{"subject":"new","description":""}"#);
        assert_eq!(s.tasks()[2].taskid, 8);
    }

    #[test]
    fn unrelated_system_message_is_ignored() {
        let mut s = TaskStore::new();
        s.apply_tool_call("TaskCreate", r#"{"subject":"a","description":""}"#);
        s.apply_system_message_text("[File: src/main.rs]\nfn main() {}");
        assert_eq!(s.tasks().len(), 1);
    }

    #[test]
    fn snapshot_text_with_multiline_subject_round_trips() {
        // Newlines / shape-breaking chars must survive JSON escaping.
        let body = r#"{
  "tasks": [
    {
      "taskid": 1,
      "status": "inprogress",
      "subject": "subject with\nembedded newline",
      "description": "desc:\n  status: not-actually-a-field"
    }
  ]
}"#;
        let text = wrap_snapshot(body, "TaskStore: 1 task(s)");
        let mut s = TaskStore::new();
        s.apply_system_message_text(&text);
        let t = &s.tasks()[0];
        assert_eq!(t.subject, "subject with\nembedded newline");
        assert_eq!(t.description, "desc:\n  status: not-actually-a-field");
    }
}

/// Snapshot format compatibility tests. The TUI deliberately re-implements a
/// stripped-down TaskStore mirror instead of depending on the Worker Task feature;
/// it only consumes task tool calls and `[Session TaskStore snapshot]` system
/// messages. These fixtures encode the Worker-owned Task snapshot JSON/text shape
/// so accidental TUI parser drift still fails locally without making `tui`
/// depend on `worker` or `tools`.
#[cfg(test)]
mod snapshot_format_contract {
    use super::*;

    /// Mirrors the envelope `Worker::try_pre_run_compact` wraps the raw
    /// snapshot text in. Hand-rolled here so the test fails loudly if
    /// the prose around the JSON fence ever shifts.
    fn wrap_pod_style(snapshot_text: &str) -> String {
        format!(
            "[Session TaskStore snapshot]\n\n{snapshot_text}\n\n\
             This is the complete session task list preserved across compaction. \
             The following TaskList tool result presents the same state through the tool lane."
        )
    }

    fn snapshot_fixture() -> &'static str {
        r#"TaskStore: 2 task(s) (pending: 0, inprogress: 1, completed: 1, deleted: 0)

```json
{
  "tasks": [
    {
      "taskid": 1,
      "status": "inprogress",
      "subject": "first",
      "description": "first desc"
    },
    {
      "taskid": 2,
      "status": "completed",
      "subject": "second",
      "description": "second desc with\nnewline"
    }
  ]
}
```"#
    }

    fn empty_snapshot_fixture() -> &'static str {
        r#"TaskStore: 0 task(s) (pending: 0, inprogress: 0, completed: 0, deleted: 0)

```json
{
  "tasks": []
}
```"#
    }

    fn status_label(s: TaskStatus) -> &'static str {
        match s {
            TaskStatus::Pending => "pending",
            TaskStatus::Inprogress => "inprogress",
            TaskStatus::Completed => "completed",
            TaskStatus::Deleted => "deleted",
        }
    }

    #[test]
    fn pod_snapshot_text_round_trips_into_tui_store() {
        let envelope = wrap_pod_style(snapshot_fixture());

        let mut downstream = TaskStore::new();
        downstream.apply_system_message_text(&envelope);

        let tasks = downstream.tasks();
        assert_eq!(tasks.len(), 2, "TUI parsed wrong number of tasks");
        assert_eq!(tasks[0].taskid, 1);
        assert_eq!(tasks[0].subject, "first");
        assert_eq!(tasks[0].description, "first desc");
        assert_eq!(status_label(tasks[0].status), "inprogress");
        assert_eq!(tasks[1].taskid, 2);
        assert_eq!(tasks[1].subject, "second");
        assert_eq!(tasks[1].description, "second desc with\nnewline");
        assert_eq!(status_label(tasks[1].status), "completed");
    }

    #[test]
    fn taskentry_field_shape_deserializes_into_tui_taskentry() {
        // A single Worker TaskEntry as JSON. Field renames like `taskid` →
        // `task_id` or status case changes surface here as serde failures or
        // wrong-status assertions.
        let json = r#"{
  "taskid": 7,
  "status": "pending",
  "subject": "subj",
  "description": "desc"
}"#;
        let parsed: TaskEntry =
            serde_json::from_str(json).expect("deserialize into tui::task::TaskEntry");
        assert_eq!(parsed.taskid, 7);
        assert_eq!(parsed.subject, "subj");
        assert_eq!(parsed.description, "desc");
        assert_eq!(status_label(parsed.status), "pending");
    }

    #[test]
    fn empty_pod_task_snapshot_is_recognised_by_tui() {
        // Edge case: a freshly initialised TaskStore still produces a valid
        // snapshot envelope. The TUI must parse it as "zero tasks", not
        // silently fall through to no-op.
        let envelope = wrap_pod_style(empty_snapshot_fixture());

        // Seed the TUI store with stale state to confirm replacement.
        let mut downstream = TaskStore::new();
        downstream.apply_tool_call("TaskCreate", r#"{"subject":"stale","description":""}"#);
        assert_eq!(downstream.tasks().len(), 1);

        downstream.apply_system_message_text(&envelope);
        assert!(downstream.is_empty());
    }
}
