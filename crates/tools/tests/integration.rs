//! Cross-tool integration tests exercising `core_builtin_tools()` end-to-end.
//!
//! `ToolServerHandle::register_tool` / `flush_pending` are `pub(crate)` in
//! agen, so from here we exercise the factories directly — the same
//! code path that `flush_pending()` runs at production time.

use std::path::Path;
use std::sync::Arc;

use agen::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolExecutionHandle,
    ToolExecutionTerminal, ToolMeta,
};
use manifest::{Permission, Scope, ScopeConfig, ScopeRule};
use serde_json::json;
use tempfile::TempDir;
use tools::{
    Tracker, core_builtin_tools, routed_builtin_tools, routed_view_image_tool, view_image_tool,
};
use workdir::{
    LocalWorkdirSession, WorkdirAttachmentAlias, WorkdirSessionHandle, WorkdirSessionRouter,
};

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
        symlink_policy: Default::default(),
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
    let fs: WorkdirSessionHandle =
        Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
    let tracker = Tracker::new();
    let reg = Registry::new(core_builtin_tools(fs, tracker, spill.path().to_path_buf()));
    (dir, spill, reg)
}

fn setup_routed() -> (
    TempDir,
    TempDir,
    TempDir,
    Arc<WorkdirSessionRouter>,
    Registry,
) {
    let left = TempDir::new().unwrap();
    let right = TempDir::new().unwrap();
    let spill = TempDir::new().unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    for (alias, dir) in [("left", &left), ("right", &right)] {
        let scope = scope_with_spill(dir.path(), spill.path());
        let session: WorkdirSessionHandle =
            Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
        router
            .attach(WorkdirAttachmentAlias::new(alias).unwrap(), session)
            .unwrap();
    }
    let definitions =
        routed_builtin_tools(router.clone(), Tracker::new(), spill.path().to_path_buf());
    (left, right, spill, router, Registry::new(definitions))
}

async fn call(tool: &Arc<dyn Tool>, input: serde_json::Value) -> agen::tool::ToolOutput {
    tool.execute(&input.to_string(), Default::default())
        .await
        .expect("tool execution failed")
}

async fn call_err(tool: &Arc<dyn Tool>, input: serde_json::Value) -> agen::tool::ToolError {
    tool.execute(&input.to_string(), Default::default())
        .await
        .expect_err("expected error")
}

#[test]
fn core_builtin_tools_registers_full_set() {
    let (_dir, _spill, reg) = setup();
    let mut names = reg.names();
    names.sort();
    assert_eq!(names, vec!["Bash", "Edit", "Glob", "Grep", "Read", "Write"]);
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

#[test]
fn routed_tool_schemas_share_optional_target_workdir() {
    let (_left, _right, _spill, router, mut reg) = setup_routed();
    reg.entries.push(routed_view_image_tool(router)());
    for (meta, _) in &reg.entries {
        let target = &meta.input_schema["properties"]["target_workdir"];
        assert!(target.is_object(), "{} lacks target_workdir", meta.name);
        assert_eq!(target["type"], json!(["string", "null"]));
    }
}

#[tokio::test]
async fn routed_tools_require_alias_only_when_attachment_set_is_ambiguous() {
    let (_left, _right, _spill, router, reg) = setup_routed();
    let error = call_err(&reg.get("Read"), json!({ "file_path": "same.txt" })).await;
    let message = error.to_string();
    assert!(message.contains("target_workdir_required"), "{message}");
    assert!(message.contains("left"), "{message}");
    assert!(message.contains("right"), "{message}");

    let unknown = call_err(
        &reg.get("Glob"),
        json!({ "target_workdir": "missing", "pattern": "*" }),
    )
    .await
    .to_string();
    assert!(unknown.contains("unknown_target_workdir"), "{unknown}");
    assert!(unknown.contains("available_aliases"), "{unknown}");

    router.close_all().await.unwrap();
}

#[tokio::test]
async fn empty_routed_surface_returns_no_attachment_error() {
    let router = Arc::new(WorkdirSessionRouter::new());
    let reg = Registry::new(routed_builtin_tools(
        router,
        Tracker::new(),
        Path::new("unused").to_path_buf(),
    ));
    let message = call_err(&reg.get("Read"), json!({ "file_path": "file.txt" }))
        .await
        .to_string();
    assert!(message.contains("no_workdir_attached"), "{message}");
}

#[tokio::test]
async fn target_alias_routes_each_tool_and_isolates_read_before_edit() {
    let (left, right, _spill, router, mut reg) = setup_routed();
    std::fs::write(left.path().join("same.txt"), "left value\n").unwrap();
    std::fs::write(right.path().join("same.txt"), "right value\n").unwrap();
    std::fs::write(left.path().join("only-left.log"), "LEFT-NEEDLE\n").unwrap();
    std::fs::write(right.path().join("only-right.log"), "RIGHT-NEEDLE\n").unwrap();

    let read = reg.get("Read");
    let left_read = call(
        &read,
        json!({ "target_workdir": "left", "file_path": "same.txt" }),
    )
    .await;
    assert!(left_read.content.unwrap().contains("left value"));

    let edit = reg.get("Edit");
    let isolated = call_err(
        &edit,
        json!({
            "target_workdir": "right",
            "file_path": "same.txt",
            "old_string": "right",
            "new_string": "edited"
        }),
    )
    .await;
    assert!(isolated.to_string().contains("has not been read"));
    call(
        &read,
        json!({ "target_workdir": "right", "file_path": "same.txt" }),
    )
    .await;
    call(
        &edit,
        json!({
            "target_workdir": "right",
            "file_path": "same.txt",
            "old_string": "right",
            "new_string": "edited"
        }),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(right.path().join("same.txt")).unwrap(),
        "edited value\n"
    );
    assert_eq!(
        std::fs::read_to_string(left.path().join("same.txt")).unwrap(),
        "left value\n"
    );

    let glob = call(
        &reg.get("Glob"),
        json!({ "target_workdir": "left", "pattern": "*.log" }),
    )
    .await;
    assert!(glob.content.unwrap().contains("only-left.log"));
    let grep = call(
        &reg.get("Grep"),
        json!({
            "target_workdir": "right",
            "pattern": "RIGHT-NEEDLE",
            "output_mode": "content"
        }),
    )
    .await;
    assert!(grep.content.unwrap().contains("only-right.log"));

    let bash = call(
        &reg.get("Bash"),
        json!({ "target_workdir": "right", "command": "pwd" }),
    )
    .await;
    assert_eq!(
        std::fs::canonicalize(bash.content.unwrap().trim()).unwrap(),
        std::fs::canonicalize(right.path()).unwrap()
    );

    let png = b"\x89PNG\r\n\x1a\nrouted";
    std::fs::write(left.path().join("image.png"), png).unwrap();
    reg.entries.push(routed_view_image_tool(router.clone())());
    let image = call(
        &reg.get("ViewImage"),
        json!({ "target_workdir": "left", "path": "image.png" }),
    )
    .await;
    let agen::tool::Attachment::Image(image) = &image.attachments[0];
    assert_eq!(image.data(), png);

    router.close_all().await.unwrap();
}

#[tokio::test]
async fn selected_attachment_capability_is_checked_without_fallback() {
    let left = TempDir::new().unwrap();
    let right = TempDir::new().unwrap();
    let spill = TempDir::new().unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("writable").unwrap(),
            Arc::new(LocalWorkdirSession::new(
                scope_with_spill(left.path(), spill.path()),
                left.path().to_path_buf(),
            )),
        )
        .unwrap();
    router
        .attach(
            WorkdirAttachmentAlias::new("readonly").unwrap(),
            Arc::new(LocalWorkdirSession::materialized_bound(
                workdir::Workdir::new("readonly"),
                right.path().to_path_buf(),
                right.path().to_path_buf(),
                manifest::SharedScope::new(scope_with_spill(right.path(), spill.path())),
                workdir::WorkdirSessionCapabilities::READ_ONLY,
            )),
        )
        .unwrap();
    let reg = Registry::new(routed_builtin_tools(
        router.clone(),
        Tracker::new(),
        spill.path().to_path_buf(),
    ));

    let message = call_err(
        &reg.get("Write"),
        json!({
            "target_workdir": "readonly",
            "file_path": "created.txt",
            "content": "must not be routed elsewhere"
        }),
    )
    .await
    .to_string();
    assert!(message.contains("does not support Write"), "{message}");
    assert!(!left.path().join("created.txt").exists());
    assert!(!right.path().join("created.txt").exists());

    router.close_all().await.unwrap();
}

#[tokio::test]
async fn routed_tools_observe_detach_after_registration() {
    let (_left, _right, _spill, router, reg) = setup_routed();
    let alias = WorkdirAttachmentAlias::new("left").unwrap();
    router.detach(&alias).await.unwrap();

    let message = call_err(
        &reg.get("Glob"),
        json!({ "target_workdir": "left", "pattern": "*" }),
    )
    .await
    .to_string();
    assert!(message.contains("unknown_target_workdir"), "{message}");
    assert!(message.contains("right"), "{message}");

    router.close_all().await.unwrap();
}

#[tokio::test]
async fn detach_and_reattach_same_alias_requires_a_fresh_read() {
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    let spill = TempDir::new().unwrap();
    std::fs::write(first.path().join("same.txt"), "same bytes\n").unwrap();
    std::fs::write(second.path().join("same.txt"), "same bytes\n").unwrap();

    let router = Arc::new(WorkdirSessionRouter::new());
    let alias = WorkdirAttachmentAlias::new("checkout").unwrap();
    router
        .attach(
            alias.clone(),
            Arc::new(LocalWorkdirSession::new(
                scope_with_spill(first.path(), spill.path()),
                first.path().to_path_buf(),
            )),
        )
        .unwrap();
    let reg = Registry::new(routed_builtin_tools(
        router.clone(),
        Tracker::new(),
        spill.path().to_path_buf(),
    ));
    call(
        &reg.get("Read"),
        json!({ "target_workdir": "checkout", "file_path": "same.txt" }),
    )
    .await;

    router.detach(&alias).await.unwrap();
    router
        .attach(
            alias.clone(),
            Arc::new(LocalWorkdirSession::new(
                scope_with_spill(second.path(), spill.path()),
                second.path().to_path_buf(),
            )),
        )
        .unwrap();

    let error = call_err(
        &reg.get("Edit"),
        json!({
            "target_workdir": "checkout",
            "file_path": "same.txt",
            "old_string": "same",
            "new_string": "changed"
        }),
    )
    .await;
    assert!(error.to_string().contains("has not been read"), "{error}");
    assert_eq!(
        std::fs::read_to_string(second.path().join("same.txt")).unwrap(),
        "same bytes\n"
    );

    router.close_all().await.unwrap();
}

#[tokio::test]
async fn view_image_reads_scoped_bytes_into_durable_tool_detail() {
    let dir = TempDir::new().unwrap();
    let spill = TempDir::new().unwrap();
    let scope = scope_with_spill(dir.path(), spill.path());
    let session: WorkdirSessionHandle =
        Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
    let png = b"\x89PNG\r\n\x1a\nprivate-image-body";
    std::fs::write(dir.path().join("image.png"), png).unwrap();
    let definition = view_image_tool(session);
    let (_meta, tool) = definition();

    let output = call(&tool, json!({ "path": "image.png" })).await;
    assert_eq!(output.attachments.len(), 1);
    let agen::tool::Attachment::Image(image) = &output.attachments[0];
    assert_eq!(image.mime_type(), "image/png");
    assert_eq!(image.data(), png);
    let serialized = serde_json::to_string(&output).unwrap();
    assert!(!serialized.contains("private-image-body"));
    assert!(serialized.contains("attachments"));
    let restored: agen::tool::ToolOutput = serde_json::from_str(&serialized).unwrap();
    let agen::tool::Attachment::Image(restored_image) = &restored.attachments[0];
    assert_eq!(restored_image.data(), png);

    let escaped = call_err(&tool, json!({ "path": "../outside.png" })).await;
    assert!(escaped.to_string().contains("scope") || escaped.to_string().contains("path"));
}

#[tokio::test]
async fn read_then_edit_then_read_roundtrip() {
    let (dir, _spill, reg) = setup();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello world\n").unwrap();
    let p = "a.txt";

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
            "file_path": file.file_name().unwrap().to_str().unwrap(),
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
    let (_dir, _spill, reg) = setup();
    let write = reg.get("Write");
    let glob = reg.get("Glob");

    for name in ["one.md", "two.md", "three.txt"] {
        call(
            &write,
            json!({
                "file_path": name,
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
async fn absolute_path_is_rejected() {
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
    // Absolute paths are rejected at the logical WorkdirSession boundary.
    let msg = format!("{err}");
    assert!(
        msg.contains("invalid logical filesystem path"),
        "unexpected: {msg}"
    );
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
            "file_path": file.file_name().unwrap().to_str().unwrap(),
            "content": "new",
        }),
    )
    .await;
    let msg = format!("{err}");
    assert!(msg.contains("has not been read"), "unexpected: {msg}");
}

#[tokio::test]
async fn shared_workdir_across_tools() {
    // The key invariant: all builtin tools share the same WorkdirSession instance,
    // so read-history set by Read is visible to Edit and Write.
    let (dir, _spill, reg) = setup();
    let file = dir.path().join("shared.txt");
    std::fs::write(&file, "one\n").unwrap();

    let read = reg.get("Read");
    let write = reg.get("Write");

    // Read via Read tool
    call(
        &read,
        json!({ "file_path": file.file_name().unwrap().to_str().unwrap() }),
    )
    .await;
    // Write via Write tool — must succeed because the shared WorkdirSession has the read
    call(
        &write,
        json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
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
            "file_path": file.file_name().unwrap().to_str().unwrap(),
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
    // Registration order from core_builtin_tools(): Read, Write, Edit, Glob, Grep, Bash
    let names: Vec<&str> = reg.entries.iter().map(|(m, _)| m.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Read", "Write", "Edit", "Glob", "Grep", "Bash",]
    );
}

// Regression: tool name capitalization matches Claude Code reference
#[test]
fn tool_names_match_reference_spec() {
    let (_dir, _spill, reg) = setup();
    for expected in ["Read", "Write", "Edit", "Glob", "Grep", "Bash"] {
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
    let fs: WorkdirSessionHandle =
        Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
    let tracker = Tracker::new();
    let reg = Registry::new(core_builtin_tools(
        fs,
        tracker.clone(),
        spill.path().to_path_buf(),
    ));

    let a = dir.path().join("a.txt");
    let _b = dir.path().join("b.txt");
    std::fs::write(&a, "one\n").unwrap();

    // Read `a` — should appear in recency.
    call(&reg.get("Read"), json!({ "file_path": "a.txt" })).await;
    // Write `b` (new file) — should appear ahead of `a`.
    call(
        &reg.get("Write"),
        json!({ "file_path": "b.txt", "content": "hello\n" }),
    )
    .await;
    // Edit `a` — should bump it back to the front.
    call(
        &reg.get("Edit"),
        json!({
            "file_path": "a.txt",
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
async fn bash_inherits_workdir_cwd() {
    // The Bash tool starts at the WorkdirSession's pwd. Without any `cd`, its
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
async fn bash_provider_output_exposes_readable_retained_path() {
    let (_dir, spill, reg) = setup();
    let bash = reg.get("Bash");
    let out = call(&bash, json!({ "command": "printf 'x%.0s' {1..20480}" })).await;
    let body = out.content.unwrap();
    assert!(body.contains("bounded WorkdirSession command output"));
    assert!(body.contains("full output saved to"));
    assert!(body.contains(spill.path().to_str().unwrap()));
    let artifact = std::fs::read_dir(spill.path())
        .unwrap()
        .next()
        .expect("retained output")
        .unwrap()
        .path();
    assert_eq!(std::fs::metadata(artifact).unwrap().len(), 20_480);
}

#[tokio::test]
async fn bash_cancellation_returns_bounded_progress_as_terminal_output() {
    let (dir, _spill, reg) = setup();
    let marker = dir.path().join("must-not-run-after-cancel");
    let command = format!(
        "printf 'before\\n'; printf 'err-before\\n' >&2; sleep 1; touch {}; printf 'after\\n'",
        marker.display()
    );
    let input = serde_json::to_string(&json!({ "command": command })).unwrap();
    let context = ToolExecutionContext::new("call-heavy", "attempt-heavy", 0);
    let bash = reg.get("Bash");
    let executing = bash.clone();
    let execution_context = context.clone();
    let execution = tokio::spawn(async move { executing.execute(&input, execution_context).await });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    bash.cancel_execution(&context)
        .await
        .expect("signal exact execution cancellation");
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), execution)
        .await
        .expect("cancelled Bash should terminate inside the Engine grace budget")
        .expect("Bash task join");

    let ToolError::Cancelled(output) = error.expect_err("cancelled command is non-success") else {
        panic!("expected typed cancellation result");
    };
    let content = output.content.expect("bounded progress output");
    assert!(
        content.contains("before"),
        "missing pre-cancel stdout: {content}"
    );
    assert!(
        content.contains("err-before"),
        "missing pre-cancel stderr: {content}"
    );
    assert!(
        !content.contains("after"),
        "post-cancel output leaked: {content}"
    );
    assert!(content.len() <= 16 * 1024, "output must remain bounded");

    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    assert!(
        !marker.exists(),
        "the cancelled command continued executing after terminal confirmation"
    );
}

#[tokio::test]
async fn bash_force_close_cleanup_stops_command_and_keeps_session_reusable() {
    let (dir, _spill, reg) = setup();
    let marker = dir.path().join("must-not-survive-force-close");
    let command = format!("sleep 1; touch {}", marker.display());
    let input = serde_json::to_string(&json!({ "command": command })).unwrap();
    let bash = reg.get("Bash");
    let context = ToolExecutionContext::new("call-force", "attempt-force", 0);
    let (handle, terminal) = ToolExecutionHandle::start(bash.clone(), input, context);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    handle.force_close();
    assert!(matches!(
        terminal.await,
        ToolExecutionTerminal::OutcomeUnknown
    ));

    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    assert!(
        !marker.exists(),
        "CommandGuard cleanup allowed a force-closed command to continue"
    );

    let output = bash
        .execute(r#"{"command":"printf 'reused'"}"#, Default::default())
        .await
        .expect("workdir session remains reusable after cleanup");
    assert_eq!(output.content.as_deref(), Some("reused"));
}

// Sanity: unused Path import guard
const _: fn() -> &'static Path = || Path::new("/");
