//! Cross-tool integration tests exercising `builtin_tools()` end-to-end.
//!
//! `ToolServerHandle::register_tool` / `flush_pending` are `pub(crate)` in
//! llm-worker, so from here we exercise the factories directly — the same
//! code path that `flush_pending()` runs at production time.

use std::path::Path;
use std::sync::Arc;

use llm_worker::tool::{Tool, ToolDefinition, ToolMeta};
use manifest::{Permission, Scope, ScopeConfig, ScopeRule};
use serde_json::json;
use tempfile::TempDir;
use tools::{ScopedFs, TaskStore, Tracker, builtin_tools};

fn scope_with_spill(workspace: &Path, spill: &Path) -> Scope {
    let base = Scope::writable(workspace).unwrap();
    let mut config = ScopeConfig {
        allow: base.allow_rules(),
        deny: base.deny_rules(),
    };
    config.allow.push(ScopeRule {
        target: spill.to_path_buf(),
        permission: Permission::Read,
        recursive: true,
    });
    Scope::from_config(&config).unwrap()
}

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

fn setup() -> (TempDir, TempDir, Registry) {
    let dir = TempDir::new().unwrap();
    let spill = TempDir::new().unwrap();
    let scope = scope_with_spill(dir.path(), spill.path());
    let fs = ScopedFs::new(scope, dir.path().to_path_buf());
    let tracker = Tracker::new();
    let reg = Registry::new(builtin_tools(
        fs,
        tracker,
        TaskStore::new(),
        spill.path().to_path_buf(),
        None,
    ));
    (dir, spill, reg)
}

async fn call(tool: &Arc<dyn Tool>, input: serde_json::Value) -> llm_worker::tool::ToolOutput {
    tool.execute(&input.to_string())
        .await
        .expect("tool execution failed")
}

async fn call_err(tool: &Arc<dyn Tool>, input: serde_json::Value) -> llm_worker::tool::ToolError {
    tool.execute(&input.to_string())
        .await
        .expect_err("expected error")
}

#[test]
fn builtin_tools_registers_full_set() {
    let (_dir, _spill, reg) = setup();
    let mut names = reg.names();
    names.sort();
    assert_eq!(
        names,
        vec![
            "Bash",
            "Edit",
            "Glob",
            "Grep",
            "Read",
            "TaskCreate",
            "TaskGet",
            "TaskList",
            "TaskUpdate",
            "WebFetch",
            "WebSearch",
            "Write"
        ]
    );
}

#[test]
fn meta_has_description_and_schema() {
    let (_dir, _spill, reg) = setup();
    for (meta, _) in &reg.entries {
        assert!(
            !meta.description.is_empty(),
            "{} missing description",
            meta.name
        );
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
    let (dir, _spill, reg) = setup();
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
    let (dir, _spill, reg) = setup();
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
    let (dir, _spill, reg) = setup();
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
    let (_dir, _spill, reg) = setup();
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
    let (dir, _spill, reg) = setup();
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
    let (dir, _spill, reg) = setup();
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
    let (dir, _spill, reg) = setup();
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
    let (_dir, _spill, reg) = setup();
    // Registration order from builtin_tools(): Read, Write, Edit, Glob, Grep, Bash, WebSearch, WebFetch, TaskCreate, TaskList, TaskGet, TaskUpdate
    let names: Vec<&str> = reg.entries.iter().map(|(m, _)| m.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "Read",
            "Write",
            "Edit",
            "Glob",
            "Grep",
            "Bash",
            "WebSearch",
            "WebFetch",
            "TaskCreate",
            "TaskList",
            "TaskGet",
            "TaskUpdate"
        ]
    );
}

// Regression: tool name capitalization matches Claude Code reference
#[test]
fn tool_names_match_reference_spec() {
    let (_dir, _spill, reg) = setup();
    for expected in [
        "Read",
        "Write",
        "Edit",
        "Glob",
        "Grep",
        "Bash",
        "WebSearch",
        "WebFetch",
        "TaskCreate",
        "TaskList",
        "TaskGet",
        "TaskUpdate",
    ] {
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
    let spill = TempDir::new().unwrap();
    let scope = scope_with_spill(dir.path(), spill.path());
    let fs = ScopedFs::new(scope, dir.path().to_path_buf());
    let tracker = Tracker::new();
    let reg = Registry::new(builtin_tools(
        fs,
        tracker.clone(),
        TaskStore::new(),
        spill.path().to_path_buf(),
        None,
    ));

    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "one\n").unwrap();

    // Read `a` — should appear in recency.
    call(
        &reg.get("Read"),
        json!({ "file_path": a.to_str().unwrap() }),
    )
    .await;
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
    assert!(
        recent[0].ends_with("a.txt"),
        "front should be a.txt: {recent:?}"
    );
    assert!(
        recent[1].ends_with("b.txt"),
        "second should be b.txt: {recent:?}"
    );
}

#[tokio::test]
async fn bash_inherits_scoped_fs_pwd() {
    // The Bash tool starts at the ScopedFs's pwd. Without any `cd`, its
    // `pwd` should canonicalize to the workspace root we set up.
    let (dir, _spill, reg) = setup();
    let bash = reg.get("Bash");
    let out = call(&bash, json!({ "command": "pwd" })).await;
    let body = out.content.unwrap();
    let actual = std::fs::canonicalize(body.trim()).unwrap();
    let expected = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn bash_spilled_file_is_readable_via_read_tool() {
    // Long Bash output spills to a path that the controller has added to
    // the readable scope. The agent should be able to Read that path
    // exactly like any in-scope file.
    let (_dir, spill, reg) = setup();
    let bash = reg.get("Bash");
    let out = call(
        &bash,
        json!({ "command": "for i in $(seq 1 200); do echo line $i; done" }),
    )
    .await;
    let body = out.content.unwrap();
    let spill_str = spill.path().to_str().unwrap();

    // Extract the spilled path from the marker line.
    let marker = body.lines().next().unwrap();
    let prefix_pos = marker
        .find(spill_str)
        .expect("marker should reference the spill dir");
    let path_end_rel = marker[prefix_pos..]
        .find(".log")
        .expect("marker should end the path with .log");
    let spilled = &marker[prefix_pos..prefix_pos + path_end_rel + 4];

    // Read the file via the Read tool — must succeed (in scope).
    let read_out = call(&reg.get("Read"), json!({ "file_path": spilled })).await;
    let read_body = read_out.content.expect("Read returned content");
    // The full 200 lines should be in the saved file even though Bash
    // returned only the tail of 80.
    assert!(
        read_body.contains("line 1\n"),
        "missing line 1: {read_body}"
    );
    assert!(read_body.contains("line 200"), "missing line 200");
}

// Sanity: unused Path import guard
const _: fn() -> &'static Path = || Path::new("/");
