//! Cross-tool integration tests exercising `builtin_tools()` end-to-end.
//!
//! `ToolServerHandle::register_tool` / `flush_pending` are `pub(crate)` in
//! llm-worker, so from here we exercise the factories directly — the same
//! code path that `flush_pending()` runs at production time.

use std::path::Path;
use std::sync::Arc;

use llm_worker::tool::{Tool, ToolDefinition, ToolMeta};
use manifest::Scope;
use serde_json::json;
use tempfile::TempDir;
use tools::{Tracker, ScopedFs, builtin_tools};

struct Registry {
    entries: Vec<(ToolMeta, Arc<dyn Tool>)>,
}

impl Registry {
    fn new(defs: Vec<ToolDefinition>) -> Self {
        let entries = defs.into_iter().map(|f| f()).collect();
        Self { entries }
    }

    fn get(&self, name: &str) -> Arc<dyn Tool> {
        self.entries
            .iter()
            .find(|(m, _)| m.name == name)
            .map(|(_, t)| Arc::clone(t))
            .unwrap_or_else(|| panic!("tool not found: {name}"))
    }

    fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|(m, _)| m.name.as_str()).collect()
    }
}

fn setup() -> (TempDir, Registry) {
    let dir = TempDir::new().unwrap();
    let fs = ScopedFs::new(Scope::new(dir.path()).unwrap());
    let tracker = Tracker::new();
    let reg = Registry::new(builtin_tools(fs, tracker));
    (dir, reg)
}

async fn call(tool: &Arc<dyn Tool>, input: serde_json::Value) -> llm_worker::tool::ToolOutput {
    tool.execute(&input.to_string())
        .await
        .expect("tool execution failed")
}

async fn call_err(
    tool: &Arc<dyn Tool>,
    input: serde_json::Value,
) -> llm_worker::tool::ToolError {
    tool.execute(&input.to_string())
        .await
        .expect_err("expected error")
}

#[test]
fn builtin_tools_registers_all_five() {
    let (_dir, reg) = setup();
    let mut names = reg.names();
    names.sort();
    assert_eq!(names, vec!["Edit", "Glob", "Grep", "Read", "Write"]);
}

#[test]
fn meta_has_description_and_schema() {
    let (_dir, reg) = setup();
    for (meta, _) in &reg.entries {
        assert!(!meta.description.is_empty(), "{} missing description", meta.name);
        // Input schema must be a JSON object
        assert!(
            meta.input_schema.is_object(),
            "{} input_schema is not an object",
            meta.name
        );
    }
}

#[tokio::test]
async fn read_then_edit_then_read_roundtrip() {
    let (dir, reg) = setup();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello world\n").unwrap();
    let p = file.to_str().unwrap();

    let read = reg.get("Read");
    let edit = reg.get("Edit");

    // Read
    let r = call(&read, json!({ "file_path": p })).await;
    assert!(r.content.unwrap().contains("hello world"));

    // Edit (unique replacement)
    let e = call(
        &edit,
        json!({
            "file_path": p,
            "old_string": "world",
            "new_string": "universe",
        }),
    )
    .await;
    assert!(e.summary.contains("1 replacement"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello universe\n");

    // Re-read reflects the change
    let r2 = call(&read, json!({ "file_path": p })).await;
    assert!(r2.content.unwrap().contains("hello universe"));
}

#[tokio::test]
async fn write_then_grep_finds_content() {
    let (dir, reg) = setup();
    let write = reg.get("Write");
    let grep = reg.get("Grep");

    let file = dir.path().join("notes.txt");
    call(
        &write,
        json!({
            "file_path": file.to_str().unwrap(),
            "content": "alpha\nNEEDLE\nomega\n",
        }),
    )
    .await;

    let g = call(
        &grep,
        json!({
            "pattern": "NEEDLE",
            "output_mode": "content",
        }),
    )
    .await;
    let body = g.content.unwrap();
    assert!(body.contains("notes.txt"));
    assert!(body.contains("NEEDLE"));
}

#[tokio::test]
async fn glob_finds_written_files() {
    let (dir, reg) = setup();
    let write = reg.get("Write");
    let glob = reg.get("Glob");

    for name in ["one.md", "two.md", "three.txt"] {
        call(
            &write,
            json!({
                "file_path": dir.path().join(name).to_str().unwrap(),
                "content": "x",
            }),
        )
        .await;
    }

    let g = call(&glob, json!({ "pattern": "*.md" })).await;
    let body = g.content.unwrap();
    assert!(body.contains("one.md"));
    assert!(body.contains("two.md"));
    assert!(!body.contains("three.txt"));
}

#[tokio::test]
async fn out_of_scope_write_is_rejected() {
    let (_dir, reg) = setup();
    let outside = TempDir::new().unwrap();
    let write = reg.get("Write");

    let err = call_err(
        &write,
        json!({
            "file_path": outside.path().join("x.txt").to_str().unwrap(),
            "content": "x",
        }),
    )
    .await;
    // ToolsError::OutOfScope → ToolError::InvalidArgument
    let msg = format!("{err}");
    assert!(msg.contains("outside allowed scope"), "unexpected: {msg}");
}

#[tokio::test]
async fn write_to_existing_without_read_fails() {
    let (dir, reg) = setup();
    let file = dir.path().join("exists.txt");
    std::fs::write(&file, "preexisting").unwrap();

    let write = reg.get("Write");
    let err = call_err(
        &write,
        json!({
            "file_path": file.to_str().unwrap(),
            "content": "new",
        }),
    )
    .await;
    let msg = format!("{err}");
    assert!(msg.contains("has not been read"), "unexpected: {msg}");
}

#[tokio::test]
async fn shared_scoped_fs_across_tools() {
    // The key invariant: all builtin tools share the same ScopedFs instance,
    // so read-history set by Read is visible to Edit and Write.
    let (dir, reg) = setup();
    let file = dir.path().join("shared.txt");
    std::fs::write(&file, "one\n").unwrap();

    let read = reg.get("Read");
    let write = reg.get("Write");

    // Read via Read tool
    call(&read, json!({ "file_path": file.to_str().unwrap() })).await;
    // Write via Write tool — must succeed because the shared ScopedFs has the read
    call(
        &write,
        json!({
            "file_path": file.to_str().unwrap(),
            "content": "two\n",
        }),
    )
    .await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "two\n");
}

#[tokio::test]
async fn edit_requires_read_across_tools() {
    let (dir, reg) = setup();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "foo\n").unwrap();

    let edit = reg.get("Edit");
    // No prior Read — Edit should fail
    let err = call_err(
        &edit,
        json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "foo",
            "new_string": "bar",
        }),
    )
    .await;
    let msg = format!("{err}");
    assert!(msg.contains("has not been read"), "unexpected: {msg}");
}

#[tokio::test]
async fn deterministic_tool_order_is_registration_order() {
    let (_dir, reg) = setup();
    // Registration order from builtin_tools(): Read, Write, Edit, Glob, Grep
    let names: Vec<&str> = reg.entries.iter().map(|(m, _)| m.name.as_str()).collect();
    assert_eq!(names, vec!["Read", "Write", "Edit", "Glob", "Grep"]);
}

// Regression: tool name capitalization matches Claude Code reference
#[test]
fn tool_names_match_reference_spec() {
    let (_dir, reg) = setup();
    for expected in ["Read", "Write", "Edit", "Glob", "Grep"] {
        assert!(
            reg.entries.iter().any(|(m, _)| m.name == expected),
            "missing tool {expected}"
        );
    }
}

#[tokio::test]
async fn tracker_recent_files_tracks_read_write_edit() {
    // Build a fresh registry that shares a tracker we can query afterwards.
    let dir = TempDir::new().unwrap();
    let fs = ScopedFs::new(Scope::new(dir.path()).unwrap());
    let tracker = Tracker::new();
    let reg = Registry::new(builtin_tools(fs, tracker.clone()));

    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "one\n").unwrap();

    // Read `a` — should appear in recency.
    call(&reg.get("Read"), json!({ "file_path": a.to_str().unwrap() })).await;
    // Write `b` (new file) — should appear ahead of `a`.
    call(
        &reg.get("Write"),
        json!({ "file_path": b.to_str().unwrap(), "content": "hello\n" }),
    )
    .await;
    // Edit `a` — should bump it back to the front.
    call(
        &reg.get("Edit"),
        json!({
            "file_path": a.to_str().unwrap(),
            "old_string": "one",
            "new_string": "two",
        }),
    )
    .await;

    let recent = tracker.recent_files(10);
    assert_eq!(recent.len(), 2);
    assert!(recent[0].ends_with("a.txt"), "front should be a.txt: {recent:?}");
    assert!(recent[1].ends_with("b.txt"), "second should be b.txt: {recent:?}");
}

// Sanity: unused Path import guard
const _: fn() -> &'static Path = || Path::new("/");
