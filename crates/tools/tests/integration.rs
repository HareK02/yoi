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
use manifest::Scope;
use serde_json::json;
use tempfile::TempDir;
use tools::{
    Tracker, core_builtin_tools, routed_builtin_tools, routed_view_image_tool, view_image_tool,
};
use workdir::{
    LocalWorkdirSession, WorkdirAttachmentAlias, WorkdirPath, WorkdirSessionHandle,
    WorkdirSessionRouter, WorkdirToolBroker, WorkdirToolScope, WorkdirToolScopePermission,
    WorkdirToolScopeRule,
};

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
    let scope = Scope::writable(dir.path()).unwrap();
    let fs: WorkdirSessionHandle =
        Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
    let tracker = Tracker::new();
    let reg = Registry::new(core_builtin_tools(
        fs,
        tracker,
        tools::bash_output::BashOutputView::new(),
    ));
    (dir, reg)
}

fn setup_routed() -> (TempDir, TempDir, Arc<WorkdirSessionRouter>, Registry) {
    let left = TempDir::new().unwrap();
    let right = TempDir::new().unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    for (alias, dir) in [("left", &left), ("right", &right)] {
        let scope = Scope::writable(dir.path()).unwrap();
        let session: WorkdirSessionHandle =
            Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
        router
            .attach(WorkdirAttachmentAlias::new(alias).unwrap(), session)
            .unwrap();
    }
    let definitions = routed_builtin_tools(
        router.clone(),
        Tracker::new(),
        tools::bash_output::BashOutputView::new(),
    );
    (left, right, router, Registry::new(definitions))
}

fn valid_gif_with_len(target_len: usize) -> Vec<u8> {
    let fixture = include_bytes!("fixtures/view-image/valid.gif");
    assert_eq!(
        fixture.last(),
        Some(&0x3b),
        "GIF fixture must end in trailer"
    );
    let mut bytes = fixture[..fixture.len() - 1].to_vec();
    bytes.extend_from_slice(&[0x21, 0xfe]);
    let mut remaining = target_len
        .checked_sub(bytes.len() + 2)
        .expect("target must leave room for comment terminator and GIF trailer");
    while remaining > 0 {
        let mut block = remaining.min(256);
        if remaining - block == 1 {
            block -= 1;
        }
        assert!(block >= 2);
        bytes.push((block - 1) as u8);
        bytes.extend(std::iter::repeat_n(b'x', block - 1));
        remaining -= block;
    }
    bytes.extend_from_slice(&[0, 0x3b]);
    assert_eq!(bytes.len(), target_len);
    bytes
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
    let (_dir, reg) = setup();
    let mut names = reg.names();
    names.sort();
    assert_eq!(names, vec!["Bash", "Edit", "Glob", "Grep", "Read", "Write"]);
}

#[test]
fn meta_has_description_and_schema() {
    let (_dir, reg) = setup();
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
    let (_left, _right, router, mut reg) = setup_routed();
    reg.entries.push(routed_view_image_tool(router)());
    for (meta, _) in &reg.entries {
        let target = &meta.input_schema["properties"]["target_workdir"];
        assert!(target.is_object(), "{} lacks target_workdir", meta.name);
        assert_eq!(target["type"], json!(["string", "null"]));
    }
}

#[tokio::test]
async fn routed_tools_require_alias_only_when_attachment_set_is_ambiguous() {
    let (_left, _right, router, reg) = setup_routed();
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
        tools::bash_output::BashOutputView::new(),
    ));
    let message = call_err(&reg.get("Read"), json!({ "file_path": "file.txt" }))
        .await
        .to_string();
    assert!(message.contains("no_workdir_attached"), "{message}");
}

#[tokio::test]
async fn target_alias_routes_each_tool_and_isolates_read_before_edit() {
    let (left, right, router, mut reg) = setup_routed();
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
        json!({ "target_workdir": "right", "command": "pwd", "cwd": "." }),
    )
    .await;
    assert_eq!(
        std::fs::canonicalize(serde_json::from_str::<serde_json::Value>(&bash.content.unwrap()).unwrap()["stdout_preview"].as_str().unwrap().trim()).unwrap(),
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
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("writable").unwrap(),
            Arc::new(LocalWorkdirSession::new(
                Scope::writable(left.path()).unwrap(),
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
                manifest::SharedScope::new(Scope::writable(right.path()).unwrap()),
                workdir::WorkdirSessionCapabilities::READ_ONLY,
            )),
        )
        .unwrap();
    let reg = Registry::new(routed_builtin_tools(
        router.clone(),
        Tracker::new(),
        tools::bash_output::BashOutputView::new(),
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
    let (_left, _right, router, reg) = setup_routed();
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
    std::fs::write(first.path().join("same.txt"), "same bytes\n").unwrap();
    std::fs::write(second.path().join("same.txt"), "same bytes\n").unwrap();

    let router = Arc::new(WorkdirSessionRouter::new());
    let alias = WorkdirAttachmentAlias::new("checkout").unwrap();
    router
        .attach(
            alias.clone(),
            Arc::new(LocalWorkdirSession::new(
                Scope::writable(first.path()).unwrap(),
                first.path().to_path_buf(),
            )),
        )
        .unwrap();
    let reg = Registry::new(routed_builtin_tools(
        router.clone(),
        Tracker::new(),
        tools::bash_output::BashOutputView::new(),
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
                Scope::writable(second.path()).unwrap(),
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

#[derive(Debug)]
struct ExternalValidatingSession {
    inner: LocalWorkdirSession,
    truncate_first_binary_response: bool,
    command_response_losses: std::sync::atomic::AtomicUsize,
    lose_terminal_only: bool,
    mutate_after_first_binary_response: Option<std::path::PathBuf>,
}

#[async_trait::async_trait]
impl workdir::WorkdirSession for ExternalValidatingSession {
    fn workdir(&self) -> &workdir::Workdir {
        self.inner.workdir()
    }

    fn capabilities(&self) -> workdir::WorkdirSessionCapabilities {
        self.inner.capabilities()
    }

    async fn stat(
        &self,
        request: workdir::StatRequest,
    ) -> Result<workdir::StatResult, workdir::WorkdirError> {
        self.inner.stat(request).await
    }

    async fn read(
        &self,
        request: workdir::ReadRequest,
    ) -> Result<workdir::ReadResult, workdir::WorkdirError> {
        let operation = workdir::http::WorkdirSessionOperation::Read(request);
        let operation = workdir::external::ExternalWorkdirOperation::try_from(operation)
            .map_err(workdir::WorkdirError::UnsupportedOperation)?;
        let workdir::http::WorkdirSessionOperation::Read(request) = operation.into_inner() else {
            unreachable!("validated Read changed operation kind")
        };
        self.inner.read(request).await
    }

    async fn read_bytes(
        &self,
        request: workdir::ReadBytesRequest,
    ) -> Result<workdir::ReadBytesResult, workdir::WorkdirError> {
        let operation = workdir::http::WorkdirSessionOperation::ReadBytes(request);
        let operation = workdir::external::ExternalWorkdirOperation::try_from(operation)
            .map_err(workdir::WorkdirError::UnsupportedOperation)?;
        let workdir::http::WorkdirSessionOperation::ReadBytes(request) = operation.into_inner()
        else {
            unreachable!("validated ReadBytes changed operation kind")
        };
        let result = self.inner.read_bytes(request).await?;
        let result = workdir::external::ExternalWorkdirOperationResult::try_from(
            workdir::http::WorkdirSessionOperationResult::ReadBytes(result),
        )
        .map_err(workdir::WorkdirError::UnsupportedOperation)?;
        let workdir::http::WorkdirSessionOperationResult::ReadBytes(mut result) =
            result.into_inner()
        else {
            unreachable!("validated ReadBytes result changed operation kind")
        };
        if result.offset == 0 {
            if let Some(path) = &self.mutate_after_first_binary_response {
                std::fs::write(path, vec![b'z'; result.total_bytes as usize]).unwrap();
            }
            if self.truncate_first_binary_response {
                result.bytes.pop();
            }
        }
        Ok(result)
    }

    async fn write(
        &self,
        request: workdir::WriteRequest,
    ) -> Result<workdir::WriteResult, workdir::WorkdirError> {
        self.inner.write(request).await
    }

    async fn edit(
        &self,
        request: workdir::EditRequest,
    ) -> Result<workdir::EditResult, workdir::WorkdirError> {
        self.inner.edit(request).await
    }

    async fn list(
        &self,
        request: workdir::ListRequest,
    ) -> Result<workdir::ListResult, workdir::WorkdirError> {
        self.inner.list(request).await
    }

    async fn glob(
        &self,
        request: workdir::GlobRequest,
    ) -> Result<workdir::GlobResult, workdir::WorkdirError> {
        self.inner.glob(request).await
    }

    async fn grep(
        &self,
        request: workdir::GrepRequest,
    ) -> Result<workdir::GrepResult, workdir::WorkdirError> {
        self.inner.grep(request).await
    }

    async fn start_command(
        &self,
        request: workdir::CommandRequest,
    ) -> Result<workdir::CommandHandle, workdir::WorkdirError> {
        self.inner.start_command(request).await
    }

    async fn command_status(
        &self,
        handle: workdir::CommandHandle,
    ) -> Result<workdir::CommandStatus, workdir::WorkdirError> {
        self.inner.command_status(handle).await
    }

    async fn command_output(
        &self,
        request: workdir::CommandOutputRequest,
    ) -> Result<workdir::CommandOutput, workdir::WorkdirError> {
        let output = self.inner.command_output(request).await?;
        if (!self.lose_terminal_only || output.terminal.is_some())
            && self
                .command_response_losses
                .fetch_update(
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
        {
            // The production provider has committed a page, but its response is lost.
            return Err(
                workdir::WorkdirError::Transport("injected lost response".into())
                    .with_failure_reason(workdir::WorkdirFailureReason::HttpTimeout),
            );
        }
        Ok(output)
    }

    async fn cancel_command(
        &self,
        handle: workdir::CommandHandle,
    ) -> Result<(), workdir::WorkdirError> {
        self.inner.cancel_command(handle).await
    }

    async fn close(&self) -> Result<(), workdir::WorkdirError> {
        self.inner.close().await
    }
}

#[tokio::test]
async fn external_view_image_uses_a_protocol_bounded_request() {
    let dir = TempDir::new().unwrap();
    let scope = Scope::writable(dir.path()).unwrap();
    let session: WorkdirSessionHandle = Arc::new(ExternalValidatingSession {
        inner: LocalWorkdirSession::new(scope, dir.path().to_path_buf()),
        truncate_first_binary_response: false,
        command_response_losses: std::sync::atomic::AtomicUsize::new(0),
        lose_terminal_only: false,
        mutate_after_first_binary_response: None,
    });
    let png = include_bytes!("fixtures/view-image/valid.png");
    assert!(png.contains(&0));
    assert!(png.iter().any(|byte| *byte >= 0x80));
    std::fs::write(dir.path().join("image.png"), png).unwrap();
    let (_meta, tool) = view_image_tool(session)();

    let output = call(&tool, json!({ "path": "image.png" })).await;
    let agen::tool::Attachment::Image(image) = &output.attachments[0];
    assert_eq!(image.mime_type(), "image/png");
    assert_eq!(image.data(), png);
}

#[tokio::test]
async fn external_view_image_rejects_truncated_and_cross_version_chunks() {
    let dir = TempDir::new().unwrap();
    let image_path = dir.path().join("image.png");
    let scope = Scope::writable(dir.path()).unwrap();
    std::fs::write(&image_path, include_bytes!("fixtures/view-image/valid.png")).unwrap();
    let truncated: WorkdirSessionHandle = Arc::new(ExternalValidatingSession {
        inner: LocalWorkdirSession::new(scope.clone(), dir.path().to_path_buf()),
        truncate_first_binary_response: true,
        command_response_losses: std::sync::atomic::AtomicUsize::new(0),
        lose_terminal_only: false,
        mutate_after_first_binary_response: None,
    });
    let (_meta, tool) = view_image_tool(truncated)();
    let error = call_err(&tool, json!({ "path": "image.png" }))
        .await
        .to_string();
    assert!(
        error.contains("truncated") || error.contains("partial"),
        "{error}"
    );

    let large = valid_gif_with_len(1024 * 1024 + 32);
    std::fs::write(&image_path, large).unwrap();
    let changing: WorkdirSessionHandle = Arc::new(ExternalValidatingSession {
        inner: LocalWorkdirSession::new(scope, dir.path().to_path_buf()),
        truncate_first_binary_response: false,
        command_response_losses: std::sync::atomic::AtomicUsize::new(0),
        lose_terminal_only: false,
        mutate_after_first_binary_response: Some(image_path),
    });
    let (_meta, tool) = view_image_tool(changing)();
    let error = call_err(&tool, json!({ "path": "image.png" }))
        .await
        .to_string();
    assert!(error.contains("changed"), "{error}");
}

#[tokio::test]
async fn view_image_enforces_the_ten_mib_boundary_without_partial_attachments() {
    const LIMIT: usize = 10 * 1024 * 1024;
    let dir = TempDir::new().unwrap();
    let scope = Scope::writable(dir.path()).unwrap();
    let session: WorkdirSessionHandle =
        Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
    let (_meta, tool) = view_image_tool(session)();
    let mut image = valid_gif_with_len(LIMIT);
    std::fs::write(dir.path().join("boundary.gif"), &image).unwrap();

    let output = call(&tool, json!({ "path": "boundary.gif" })).await;
    let agen::tool::Attachment::Image(attachment) = &output.attachments[0];
    assert_eq!(attachment.mime_type(), "image/gif");
    assert_eq!(attachment.data(), image);

    image.push(0);
    std::fs::write(dir.path().join("boundary.gif"), image).unwrap();
    let error = call_err(&tool, json!({ "path": "boundary.gif" }))
        .await
        .to_string();
    assert!(error.contains("10485760-byte limit"), "{error}");
}

#[tokio::test]
async fn view_image_attaches_valid_supported_image_fixtures() {
    let dir = TempDir::new().unwrap();
    let scope = Scope::writable(dir.path()).unwrap();
    let session: WorkdirSessionHandle =
        Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
    let (_meta, tool) = view_image_tool(session)();
    let fixtures: [(&str, &[u8], &str); 4] = [
        (
            "valid.png",
            include_bytes!("fixtures/view-image/valid.png"),
            "image/png",
        ),
        (
            "valid.jpg",
            include_bytes!("fixtures/view-image/valid.jpg"),
            "image/jpeg",
        ),
        (
            "valid.gif",
            include_bytes!("fixtures/view-image/valid.gif"),
            "image/gif",
        ),
        (
            "valid.webp",
            include_bytes!("fixtures/view-image/valid.webp"),
            "image/webp",
        ),
    ];

    for (name, fixture, mime) in fixtures {
        std::fs::write(dir.path().join(name), fixture).unwrap();
        let output = call(&tool, json!({ "path": name })).await;
        let agen::tool::Attachment::Image(image) = &output.attachments[0];
        assert_eq!(image.mime_type(), mime);
        assert_eq!(image.data(), fixture);
    }
}

#[tokio::test]
async fn delegated_view_image_preserves_caller_relative_path_identity() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("docs")).unwrap();
    let fixture = include_bytes!("fixtures/view-image/valid.png");
    std::fs::write(dir.path().join("docs/image.png"), fixture).unwrap();
    let source: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::new(
        Scope::writable(dir.path()).unwrap(),
        dir.path().to_path_buf(),
    ));
    let broker = WorkdirToolBroker::new(source);
    let lease = broker
        .scope(WorkdirToolScope {
            rules: vec![WorkdirToolScopeRule {
                target: WorkdirPath::new("docs").unwrap(),
                permission: WorkdirToolScopePermission::Read,
                recursive: true,
                symlink_policy: Default::default(),
            }],
            cwd: WorkdirPath::new("docs").unwrap(),
            command: false,
        })
        .await
        .unwrap();
    let (_meta, tool) = view_image_tool(lease.tool_session())();

    let output = call(&tool, json!({ "path": "image.png" })).await;
    assert!(output.summary.contains("image.png"));
    assert!(!output.summary.contains("docs/image.png"));
    let agen::tool::Attachment::Image(image) = &output.attachments[0];
    assert_eq!(image.data(), fixture);

    lease.close().await.unwrap();
    broker.close().await.unwrap();
}

#[tokio::test]
async fn view_image_reads_scoped_bytes_into_durable_tool_detail() {
    let dir = TempDir::new().unwrap();
    let scope = Scope::writable(dir.path()).unwrap();
    let session: WorkdirSessionHandle =
        Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
    let png = include_bytes!("fixtures/view-image/valid.png");
    std::fs::write(dir.path().join("image.png"), png).unwrap();
    let definition = view_image_tool(session);
    let (_meta, tool) = definition();

    let output = call(&tool, json!({ "path": "image.png" })).await;
    assert_eq!(output.attachments.len(), 1);
    let agen::tool::Attachment::Image(image) = &output.attachments[0];
    assert_eq!(image.mime_type(), "image/png");
    assert_eq!(image.data(), png);
    let serialized = serde_json::to_string(&output).unwrap();
    assert!(serialized.contains("attachments"));
    let restored: agen::tool::ToolOutput = serde_json::from_str(&serialized).unwrap();
    let agen::tool::Attachment::Image(restored_image) = &restored.attachments[0];
    assert_eq!(restored_image.data(), png);

    let escaped = call_err(&tool, json!({ "path": "../outside.png" })).await;
    assert!(escaped.to_string().contains("scope") || escaped.to_string().contains("path"));
}

#[tokio::test]
async fn read_then_edit_then_read_roundtrip() {
    let (dir, reg) = setup();
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
    let (dir, reg) = setup();
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
    let (_dir, reg) = setup();
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
    // Absolute paths are rejected at the logical WorkdirSession boundary.
    let msg = format!("{err}");
    assert!(
        msg.contains("invalid logical filesystem path"),
        "unexpected: {msg}"
    );
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
    let (dir, reg) = setup();
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
    let (dir, reg) = setup();
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
    let (_dir, reg) = setup();
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
    let (_dir, reg) = setup();
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
    let scope = Scope::writable(dir.path()).unwrap();
    let fs: WorkdirSessionHandle =
        Arc::new(LocalWorkdirSession::new(scope, dir.path().to_path_buf()));
    let tracker = Tracker::new();
    let reg = Registry::new(core_builtin_tools(
        fs,
        tracker.clone(),
        tools::bash_output::BashOutputView::new(),
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
    let (dir, reg) = setup();
    let bash = reg.get("Bash");
    let out = call(&bash, json!({ "command": "pwd", "cwd": "." })).await;
    let body = out.content.unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    let actual = std::fs::canonicalize(parsed["stdout_preview"].as_str().unwrap().trim()).unwrap();
    let expected = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(actual, expected);
}

fn output_fixture() -> (
    TempDir,
    WorkdirSessionHandle,
    tools::bash_output::BashOutputView,
    Arc<dyn Tool>,
) {
    let root = TempDir::new().unwrap();
    let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::new(
        Scope::writable(root.path()).unwrap(),
        root.path().into(),
    ));
    let view = tools::bash_output::BashOutputView::new();
    let (_, bash) = tools::bash_tool(session.clone(), view.clone())();
    (root, session, view, bash)
}

#[tokio::test]
async fn bash_output_is_retained_without_a_filesystem_grant_or_telemetry_subscriber() {
    let (root, _session, view, bash) = output_fixture();
    let out = call(
        &bash,
        json!({ "command": "printf 'x%.0s' {1..20480}; printf '\\377€' >&2; exit 7", "cwd": "." }),
    )
    .await;
    let body: serde_json::Value = serde_json::from_str(&out.content.unwrap()).unwrap();
    assert_eq!(body["output"]["terminal"]["exit_code"], 7);
    assert_eq!(body["preview_omitted"], true);
    assert_eq!(body["output"]["storage_truncated"], false);
    let id = body["output"]["id"].as_str().unwrap();
    assert_eq!(
        view.read(id, workdir::CommandStream::Stdout, 0, 65536)
            .unwrap(),
        vec![b'x'; 20480]
    );
    assert_eq!(
        view.read(id, workdir::CommandStream::Stderr, 0, 65536)
            .unwrap(),
        [vec![255], "€".as_bytes().to_vec()].concat()
    );
    assert!(
        body["stdout_entry"]
            .as_str()
            .unwrap()
            .starts_with("/bash-output/")
    );
    assert!(!body.to_string().contains(root.path().to_str().unwrap()));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

async fn await_command_output(
    events: &mut tokio::sync::broadcast::Receiver<workdir::CommandEvent>,
) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if matches!(
                events.recv().await.unwrap(),
                workdir::CommandEvent::Output { .. }
            ) {
                break;
            }
        }
    })
    .await
    .expect("command output readiness");
}
async fn await_command_terminal(
    events: &mut tokio::sync::broadcast::Receiver<workdir::CommandEvent>,
) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let workdir::CommandEvent::Terminal { status, .. } = events.recv().await.unwrap() {
                assert_eq!(status, workdir::CommandStatus::Cancelled);
                break;
            }
        }
    })
    .await
    .expect("command terminal fence");
}

#[tokio::test]
async fn bash_cancellation_returns_bounded_progress_as_terminal_output() {
    let (_root, session, _view, bash) = output_fixture();
    let mut events = session.subscribe_command_events().unwrap();
    let context = ToolExecutionContext::new("call-heavy", "attempt-heavy", 0);
    let executing = bash.clone();
    let execution_context = context.clone();
    let execution = tokio::spawn(async move {
        executing.execute(r#"{"command":"printf 'before'; printf 'err-before' >&2; while :; do :; done","cwd":"."}"#, execution_context).await
    });
    await_command_output(&mut events).await;
    bash.cancel_execution(&context).await.unwrap();
    let ToolError::Cancelled(output) =
        tokio::time::timeout(std::time::Duration::from_secs(5), execution)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
    else {
        panic!("expected cancellation");
    };
    let body: serde_json::Value = serde_json::from_str(&output.content.unwrap()).unwrap();
    assert_eq!(body["stdout_preview"], "before");
    assert_eq!(body["stderr_preview"], "err-before");
    assert_eq!(body["output"]["terminal"]["status"], "cancelled");
    assert_eq!(body["output"]["terminal"]["transport_complete"], true);
}

#[tokio::test]
async fn bash_force_close_cleanup_stops_command_and_keeps_session_reusable() {
    let (_root, session, _view, bash) = output_fixture();
    let mut events = session.subscribe_command_events().unwrap();
    let context = ToolExecutionContext::new("call-force", "attempt-force", 0);
    let (handle, terminal) = ToolExecutionHandle::start(
        bash.clone(),
        r#"{"command":"printf ready; while :; do :; done","cwd":"."}"#.into(),
        context,
    );
    await_command_output(&mut events).await;
    handle.force_close();
    assert!(matches!(
        terminal.await,
        ToolExecutionTerminal::OutcomeUnknown
    ));
    await_command_terminal(&mut events).await;
    let output = call(&bash, json!({"command":"printf reused", "cwd":"."})).await;
    let body: serde_json::Value = serde_json::from_str(&output.content.unwrap()).unwrap();
    assert_eq!(body["stdout_preview"], "reused");
}

// Sanity: unused Path import guard
const _: fn() -> &'static Path = || Path::new("/");

#[tokio::test]
async fn parent_and_sibling_broker_commands_keep_output_ownership_after_revoke() {
    let (root, source, parent_view, parent_bash) = output_fixture();
    let broker = WorkdirToolBroker::new(source);
    let mut leases = Vec::new();
    let mut views = Vec::new();
    let mut commands = Vec::new();
    for name in ["left", "right"] {
        std::fs::create_dir(root.path().join(name)).unwrap();
        let lease = Arc::new(
            broker
                .scope(WorkdirToolScope {
                    rules: vec![WorkdirToolScopeRule {
                        target: WorkdirPath::new(name).unwrap(),
                        permission: WorkdirToolScopePermission::Write,
                        recursive: true,
                        symlink_policy: Default::default(),
                    }],
                    cwd: WorkdirPath::new(name).unwrap(),
                    command: true,
                })
                .await
                .unwrap(),
        );
        let authority = lease.clone();
        let view = parent_view.child(Arc::new(move || authority.is_active()));
        let (_, bash) = tools::bash_tool(lease.tool_session(), view.clone())();
        leases.push(lease);
        views.push(view);
        commands.push(bash);
    }
    let left_input =
        json!({"command":"printf 'left-%.0s' {1..10000}; printf left-err >&2", "cwd":"."});
    let right_input =
        json!({"command":"printf 'right-%.0s' {1..10000}; printf right-err >&2", "cwd":"."});
    let parent_input = json!({"command":"printf parent; printf parent-err >&2", "cwd":"."});
    let (left, right, parent) = tokio::join!(
        call(&commands[0], left_input),
        call(&commands[1], right_input),
        call(&parent_bash, parent_input)
    );
    let outputs = [left, right, parent]
        .map(|out| serde_json::from_str::<serde_json::Value>(&out.content.unwrap()).unwrap());
    for (index, label) in ["left", "right", "parent"].iter().enumerate() {
        let id = outputs[index]["output"]["id"].as_str().unwrap();
        let bytes = parent_view
            .read(id, workdir::CommandStream::Stderr, 0, 65536)
            .unwrap();
        assert_eq!(bytes, format!("{label}-err").as_bytes());
        for (child_index, view) in views.iter().enumerate() {
            assert_eq!(view.metadata(id).is_some(), child_index == index);
        }
    }
    let left_id = outputs[0]["output"]["id"].as_str().unwrap();
    leases[0].close().await.unwrap();
    assert!(views[0].metadata(left_id).is_none());
    assert!(parent_view.metadata(left_id).is_some());
    assert!(
        views[1]
            .metadata(outputs[1]["output"]["id"].as_str().unwrap())
            .is_some()
    );
    leases[1].close().await.unwrap();
    broker.close().await.unwrap();
    parent_view.revoke();
    assert!(parent_view.metadata(left_id).is_none());
}

#[tokio::test]
async fn utf8_quota_boundary_preserves_raw_prefix_and_reports_saved_vs_seen() {
    let (_root, _session, view, bash) = output_fixture();
    let out = call(
        &bash,
        json!({"command":"printf '%4194303s' ''; printf '€'", "cwd":"."}),
    )
    .await;
    let body: serde_json::Value = serde_json::from_str(&out.content.unwrap()).unwrap();
    let id = body["output"]["id"].as_str().unwrap();
    let saved = body["output"]["stdout_saved"].as_u64().unwrap() as usize;
    assert_eq!(saved, 4 * 1024 * 1024);
    assert_eq!(body["output"]["stdout_seen"], 4 * 1024 * 1024 + 2);
    assert_eq!(body["output"]["stderr_seen"], 0);
    assert_eq!(body["output"]["storage_truncated"], true);
    assert_eq!(
        view.read(id, workdir::CommandStream::Stdout, saved - 1, 10)
            .unwrap(),
        vec![0xe2]
    );
    assert!(
        view.line_window(id, workdir::CommandStream::Stdout, 0, 1)
            .unwrap()
            .is_err()
    );
    assert!(out.summary.starts_with("$ "));
}

#[tokio::test]
async fn timeout_terminal_fences_retained_stdout_and_stderr() {
    let (_root, _session, view, bash) = output_fixture();
    let error = tokio::time::timeout(std::time::Duration::from_secs(5), bash.execute(r#"{"command":"printf before; printf err-before >&2; while :; do :; done","cwd":".","timeout":1}"#, Default::default())).await.unwrap().unwrap_err();
    let ToolError::Interrupted(out) = error else {
        panic!("expected timeout output");
    };
    let body: serde_json::Value = serde_json::from_str(&out.content.unwrap()).unwrap();
    let id = body["output"]["id"].as_str().unwrap();
    assert_eq!(body["output"]["terminal"]["status"], "timed_out");
    assert_eq!(body["output"]["terminal"]["transport_complete"], true);
    assert_eq!(
        view.read(id, workdir::CommandStream::Stdout, 0, 65536)
            .unwrap(),
        b"before"
    );
    assert_eq!(
        view.read(id, workdir::CommandStream::Stderr, 0, 65536)
            .unwrap(),
        b"err-before"
    );
}

// Integration: real process pipes and provider replay into the production Bash collector.
// Only response delivery is fault-injected; Runtime's HTTP suite covers actual wire routing.
#[tokio::test]
async fn lost_page_and_terminal_responses_replay_without_restarting_or_double_appending() {
    for lose_terminal_only in [false, true] {
        let dir = TempDir::new().unwrap();
        let session = Arc::new(ExternalValidatingSession {
            inner: LocalWorkdirSession::new(
                Scope::writable(dir.path()).unwrap(),
                dir.path().into(),
            ),
            truncate_first_binary_response: false,
            mutate_after_first_binary_response: None,
            command_response_losses: std::sync::atomic::AtomicUsize::new(1),
            lose_terminal_only,
        });
        let view = tools::bash_output::BashOutputView::new();
        let definitions = core_builtin_tools(session.clone(), Tracker::new(), view.clone());
        let bash = Registry::new(definitions).get("Bash");
        let out = call(&bash, json!({"cwd":".","command":"printf once >> effect.txt; printf 'x%.0s' {1..200000}; printf err >&2; exit 3"})).await;
        let body: serde_json::Value = serde_json::from_str(&out.content.unwrap()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("effect.txt")).unwrap(),
            b"once"
        );
        assert_eq!(
            session
                .command_response_losses
                .load(std::sync::atomic::Ordering::Acquire),
            0
        );
        assert_eq!(body["output"]["stdout_saved"], 200000);
        assert_eq!(body["output"]["stdout_delivered"], 200000);
        assert_eq!(body["output"]["stderr_saved"], 3);
        assert_eq!(body["output"]["terminal"]["exit_code"], 3);
        assert_eq!(body["output"]["terminal"]["transport_complete"], true);
        assert_eq!(body["output"]["storage_truncated"], false);
        let id = body["output"]["id"].as_str().unwrap();
        for offset in (0..200000).step_by(65536) {
            assert_eq!(
                view.read(id, workdir::CommandStream::Stdout, offset, 65536)
                    .unwrap(),
                vec![b'x'; (200000 - offset).min(65536)]
            );
        }
        use workdir::WorkdirSession as _;
        session.close().await.unwrap();
    }
}
