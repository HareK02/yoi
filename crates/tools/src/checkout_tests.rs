use super::*;
use manifest::Scope;
use serde_json::json;
use tempfile::TempDir;
use workdir::{LocalWorkdirSession, WorkdirAttachmentAlias};

struct Fixture {
    dir: TempDir,
    router: Arc<WorkdirSessionRouter>,
    tracker: Tracker,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let router = Arc::new(WorkdirSessionRouter::new());
        let fixture = Self {
            dir,
            router,
            tracker: Tracker::new(),
        };
        fixture.attach("main");
        fixture
    }

    fn attach(&self, alias: &str) {
        self.router
            .attach(
                WorkdirAttachmentAlias::new(alias).unwrap(),
                Arc::new(LocalWorkdirSession::new(
                    Scope::writable(self.dir.path()).unwrap(),
                    self.dir.path().to_path_buf(),
                )),
            )
            .unwrap();
    }

    async fn observation(&self, alias: &str, path: &str) -> (u64, Vec<u8>) {
        let selected = self.router.resolve(Some(alias)).unwrap();
        let observation = selected
            .session
            .checkout_observe(WorkdirPath::new(path).unwrap())
            .await
            .unwrap();
        (selected.generation, observation.validator)
    }

    async fn native(
        &self,
        alias: &str,
        path: &str,
        name: &str,
        arguments: Value,
    ) -> Result<CheckoutToolOutput, ToolError> {
        let (generation, validator) = self.observation(alias, path).await;
        execute_checkout_tool(
            self.router.clone(),
            self.tracker.clone(),
            alias,
            generation,
            WorkdirPath::new(path).unwrap(),
            validator,
            name,
            arguments,
            Default::default(),
        )
        .await
    }

    async fn normal(&self, name: &str, arguments: Value) -> Result<ToolOutput, ToolError> {
        let definitions = crate::routed_builtin_tools(
            self.router.clone(),
            self.tracker.clone(),
            Default::default(),
        );
        for definition in definitions {
            let (meta, tool) = definition();
            if meta.name == name {
                return tool
                    .execute(&arguments.to_string(), Default::default())
                    .await;
            }
        }
        panic!("missing Tool {name}")
    }
}

#[derive(Debug)]
struct ListProvider {
    workdir: workdir::Workdir,
    capabilities: workdir::WorkdirSessionCapabilities,
    result: workdir::CheckoutSearchResult,
    change_validator: bool,
    expected_after: Option<workdir::ListCursor>,
    searched: std::sync::atomic::AtomicBool,
}

// This boundary adapter exposes only checked metadata/search. Any file content
// read, ordinary list fallback, or command execution is a test failure.
#[async_trait::async_trait]
impl workdir::WorkdirSession for ListProvider {
    fn workdir(&self) -> &workdir::Workdir {
        &self.workdir
    }
    fn capabilities(&self) -> workdir::WorkdirSessionCapabilities {
        self.capabilities
    }
    async fn checkout_observe(
        &self,
        path: WorkdirPath,
    ) -> Result<workdir::CheckoutObservation, workdir::WorkdirError> {
        assert_eq!(
            path.as_str(),
            "src",
            "List observes only its bound directory"
        );
        Ok(workdir::CheckoutObservation {
            path,
            kind: workdir::EntryKind::Directory,
            size: 0,
            validator: vec![u8::from(
                self.change_validator && self.searched.load(std::sync::atomic::Ordering::SeqCst),
            )],
            capabilities: self.capabilities,
        })
    }
    async fn checkout_search(
        &self,
        request: workdir::CheckoutSearchRequest,
    ) -> Result<workdir::CheckoutSearchResult, workdir::WorkdirError> {
        let workdir::CheckoutSearchOperation::List(list) = request.operation else {
            panic!("native List must dispatch a typed List request");
        };
        assert_eq!(list.after, self.expected_after);
        assert!(
            !self
                .searched
                .swap(true, std::sync::atomic::Ordering::SeqCst),
            "List must not prefetch another page"
        );
        Ok(self.result.clone())
    }
    async fn stat(
        &self,
        _: workdir::StatRequest,
    ) -> Result<workdir::StatResult, workdir::WorkdirError> {
        panic!("native List must not call stat");
    }
    async fn read(
        &self,
        _: workdir::ReadRequest,
    ) -> Result<workdir::ReadResult, workdir::WorkdirError> {
        panic!("native List must not read file content");
    }
    async fn write(
        &self,
        _: workdir::WriteRequest,
    ) -> Result<workdir::WriteResult, workdir::WorkdirError> {
        panic!("native List must not write");
    }
    async fn edit(
        &self,
        _: workdir::EditRequest,
    ) -> Result<workdir::EditResult, workdir::WorkdirError> {
        panic!("native List must not edit");
    }
    async fn list(
        &self,
        _: workdir::ListRequest,
    ) -> Result<workdir::ListResult, workdir::WorkdirError> {
        panic!("native List must use checkout_search, not ordinary list");
    }
    async fn glob(
        &self,
        _: workdir::GlobRequest,
    ) -> Result<workdir::GlobResult, workdir::WorkdirError> {
        panic!("native List must not glob");
    }
    async fn grep(
        &self,
        _: workdir::GrepRequest,
    ) -> Result<workdir::GrepResult, workdir::WorkdirError> {
        panic!("native List must not grep");
    }
    async fn start_command(
        &self,
        _: workdir::CommandRequest,
    ) -> Result<workdir::CommandHandle, workdir::WorkdirError> {
        panic!("native List must not execute commands");
    }
    async fn command_status(
        &self,
        _: workdir::CommandHandle,
    ) -> Result<workdir::CommandStatus, workdir::WorkdirError> {
        panic!("native List must not use commands");
    }
    async fn command_output(
        &self,
        _: workdir::CommandOutputRequest,
    ) -> Result<workdir::CommandOutput, workdir::WorkdirError> {
        panic!("native List must not use commands");
    }
    async fn cancel_command(&self, _: workdir::CommandHandle) -> Result<(), workdir::WorkdirError> {
        panic!("native List must not use commands");
    }
    async fn close(&self) -> Result<(), workdir::WorkdirError> {
        Ok(())
    }
}

async fn list_with_provider(
    capabilities: workdir::WorkdirSessionCapabilities,
    result: workdir::CheckoutSearchResult,
    change_validator: bool,
) -> Result<CheckoutToolOutput, ToolError> {
    list_with_provider_after(capabilities, result, change_validator, None).await
}

async fn list_with_provider_after(
    capabilities: workdir::WorkdirSessionCapabilities,
    result: workdir::CheckoutSearchResult,
    change_validator: bool,
    after: Option<workdir::ListCursor>,
) -> Result<CheckoutToolOutput, ToolError> {
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("selected").unwrap(),
            Arc::new(ListProvider {
                workdir: workdir::Workdir::new("list-test"),
                capabilities,
                result,
                change_validator,
                expected_after: after.clone(),
                searched: std::sync::atomic::AtomicBool::new(false),
            }),
        )
        .unwrap();
    let generation = router.resolve(Some("selected")).unwrap().generation;
    execute_checkout_tool(
        router,
        Tracker::new(),
        "selected",
        generation,
        WorkdirPath::new("src").unwrap(),
        vec![0],
        "List",
        json!({"limit": 1, "after": after}),
        Default::default(),
    )
    .await
}

fn list_page() -> workdir::ListResult {
    workdir::ListResult {
        entries: vec![workdir::ListEntry {
            path: WorkdirPath::new("src/a.txt").unwrap(),
            kind: workdir::EntryKind::File,
            size: 5,
        }],
        total_entries: 2,
        total_bytes: 10,
        truncated: true,
        next_after: Some(fs_operation::ListCursor {
            kind: workdir::EntryKind::File,
            path: WorkdirPath::new("src/a.txt").unwrap(),
        }),
    }
}

#[tokio::test]
async fn checkout_list_uses_read_capability_and_checked_search_without_prefetch() {
    let read_only = workdir::WorkdirSessionCapabilities::EMPTY.with(WorkdirSessionCapability::Read);
    let page = list_page();
    let result = list_with_provider(
        read_only,
        workdir::CheckoutSearchResult::List(page.clone()),
        false,
    )
    .await
    .unwrap();
    assert_eq!(result.listing, Some(page));
    assert_eq!(result.validator, Some(vec![0]));
    let no_read = workdir::WorkdirSessionCapabilities::EMPTY.with(WorkdirSessionCapability::Glob);
    let error = list_with_provider(
        no_read,
        workdir::CheckoutSearchResult::List(list_page()),
        false,
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(error, ToolError::InvalidArgument(_)));
    assert!(error.to_string().contains("Read"));
}

#[tokio::test]
async fn checkout_list_rejects_changed_post_search_directory_validator() {
    let error = list_with_provider(
        workdir::WorkdirSessionCapabilities::READ_ONLY,
        workdir::CheckoutSearchResult::List(list_page()),
        true,
    )
    .await
    .err()
    .unwrap();
    assert_checkout_stale(error);
}

#[tokio::test]
async fn checkout_list_rejects_provider_pages_that_escape_or_exceed_bounds() {
    let mut outside = list_page();
    outside.entries[0].path = WorkdirPath::new("outside.txt").unwrap();
    let mut nested = list_page();
    nested.entries[0].path = WorkdirPath::new("src/nested/a.txt").unwrap();
    let mut cursor_outside = list_page();
    cursor_outside.next_after.as_mut().unwrap().path = WorkdirPath::new("outside.txt").unwrap();
    let mut cursor_kind = list_page();
    cursor_kind.next_after.as_mut().unwrap().kind = workdir::EntryKind::Directory;
    let mut oversized = list_page();
    oversized.entries.push(oversized.entries[0].clone());
    for result in [
        workdir::CheckoutSearchResult::List(outside),
        workdir::CheckoutSearchResult::List(nested),
        workdir::CheckoutSearchResult::List(cursor_outside),
        workdir::CheckoutSearchResult::List(cursor_kind),
        workdir::CheckoutSearchResult::List(oversized),
        workdir::CheckoutSearchResult::Glob(workdir::GlobResult {
            paths: vec![],
            truncated: false,
        }),
    ] {
        let error = list_with_provider(
            workdir::WorkdirSessionCapabilities::READ_ONLY,
            result,
            false,
        )
        .await
        .err()
        .unwrap();
        assert_checkout_code(error, "checkout_unavailable");
    }
}

#[tokio::test]
async fn checkout_list_special_entry_cursors_preserve_kind_and_continue_the_page() {
    // Provider-only special entries stay path coordinates: the adapter forbids
    // content reads and does not offer direct observations for these entries.
    for kind in [workdir::EntryKind::Other, workdir::EntryKind::Symlink] {
        let mut first_page = list_page();
        first_page.entries[0].kind = kind;
        first_page.next_after.as_mut().unwrap().kind = kind;
        let first = list_with_provider(
            workdir::WorkdirSessionCapabilities::READ_ONLY,
            workdir::CheckoutSearchResult::List(first_page),
            false,
        )
        .await
        .unwrap()
        .listing
        .unwrap();
        assert_eq!(first.entries[0].kind, kind);
        let after = first.next_after.unwrap();
        assert_eq!(after.kind, kind);
        let mut second_page = list_page();
        second_page.entries[0].path = WorkdirPath::new("src/z.txt").unwrap();
        second_page.truncated = false;
        second_page.next_after = None;
        let second = list_with_provider_after(
            workdir::WorkdirSessionCapabilities::READ_ONLY,
            workdir::CheckoutSearchResult::List(second_page.clone()),
            false,
            Some(after),
        )
        .await
        .unwrap()
        .listing
        .unwrap();
        assert_eq!(second, second_page);
    }
}

#[tokio::test]
async fn checkout_list_rejects_missing_or_mismatched_truncated_continuations() {
    let mut missing = list_page();
    missing.next_after = None;
    let mut empty = list_page();
    empty.entries.clear();
    let mut wrong_path = list_page();
    wrong_path.next_after.as_mut().unwrap().path = WorkdirPath::new("src/z.txt").unwrap();
    let mut wrong_group = list_page();
    wrong_group.next_after.as_mut().unwrap().kind = workdir::EntryKind::Directory;
    let mut untruncated = list_page();
    untruncated.truncated = false;
    for page in [missing, empty, wrong_path, wrong_group, untruncated] {
        let error = list_with_provider(
            workdir::WorkdirSessionCapabilities::READ_ONLY,
            workdir::CheckoutSearchResult::List(page),
            false,
        )
        .await
        .err()
        .unwrap();
        assert_checkout_code(error, "checkout_unavailable");
    }
}

fn assert_checkout_stale(error: ToolError) {
    assert_checkout_code(error, "checkout_stale");
}

fn assert_checkout_code(error: ToolError, expected: &str) {
    match error {
        ToolError::StructuredConflict { code, .. } => assert_eq!(code, expected),
        other => panic!("expected {expected} structured conflict, got {other:?}"),
    }
}

#[test]
fn checkout_known_refusals_have_typed_codes_without_changing_normal_tools() {
    use workdir::{WorkdirError, WorkdirSessionCapability};
    let cases = [
        (WorkdirError::Denied("denied".into()), "checkout_denied"),
        (
            WorkdirError::DenialContext {
                reason: workdir::WorkdirDenialReason::OsPermissionDenied,
                source: Box::new(WorkdirError::OutOfScope("outside".into())),
            },
            "checkout_denied",
        ),
        (
            WorkdirError::OutOfScope("outside".into()),
            "checkout_denied",
        ),
        (
            WorkdirError::ReadOnly("read only".into()),
            "checkout_denied",
        ),
        (
            WorkdirError::Unsupported(WorkdirSessionCapability::Write),
            "checkout_denied",
        ),
        (
            WorkdirError::InvalidArgument("bad argument".into()),
            "checkout_invalid",
        ),
        (
            WorkdirError::InvalidPath("bad path".into()),
            "checkout_invalid",
        ),
        (
            WorkdirError::Unavailable("budget exhausted".into()),
            "checkout_unavailable",
        ),
        (WorkdirError::NotFound("missing".into()), "checkout_stale"),
        (WorkdirError::SessionClosed, "checkout_unavailable"),
        (
            WorkdirError::UnsupportedOperation("unsupported".into()),
            "checkout_denied",
        ),
        (
            WorkdirError::UnknownCommand("missing command".into()),
            "checkout_invalid",
        ),
        (
            WorkdirError::RelativePath("relative".into()),
            "checkout_invalid",
        ),
        (
            WorkdirError::IsDirectory("directory".into()),
            "checkout_invalid",
        ),
        (
            WorkdirError::InvalidGlob("bad glob".into()),
            "checkout_invalid",
        ),
        (
            WorkdirError::InvalidRegex("bad regex".into()),
            "checkout_invalid",
        ),
    ];
    for (error, code) in cases {
        assert_checkout_code(crate::file_target::checked_error(error), code);
    }
    assert!(matches!(
        ToolError::from(ToolsError::from(WorkdirError::Unavailable(
            "unavailable".into()
        ))),
        ToolError::ExecutionFailed(_)
    ));
    assert!(matches!(
        ToolError::from(ToolsError::from(WorkdirError::Unsupported(
            WorkdirSessionCapability::Write
        ))),
        ToolError::InvalidArgument(_)
    ));
}

#[test]
fn checkout_conflict_classification_preserves_normal_tools_and_unknown_outcomes() {
    assert_checkout_stale(crate::file_target::checked_error(
        workdir::WorkdirError::Conflict("a.txt".into()),
    ));
    assert_checkout_stale(stale_search());
    assert!(matches!(
        ToolError::from(ToolsError::from(workdir::WorkdirError::Conflict(
            "a.txt".into()
        ))),
        ToolError::ExecutionFailed(_)
    ));
    assert!(matches!(
        crate::file_target::checked_error(workdir::WorkdirError::OutcomeUnknown(
            "may have committed".into()
        )),
        ToolError::ExecutionFailed(_)
    ));
}

#[test]
fn checkout_ambiguous_transport_and_operation_failures_remain_unknown() {
    use workdir::WorkdirError;
    // Even refusal-looking text in an ambiguous transport error is not proof
    // that the provider rejected the request before any effects.
    for error in [
        WorkdirError::Transport("denied (ambiguous response)".into()),
        WorkdirError::OperationFailed,
    ] {
        assert!(matches!(
            crate::file_target::checked_error(error),
            ToolError::ExecutionFailed(_)
        ));
    }
}

#[test]
fn checkout_mismatched_readonly_results_are_unavailable_not_stale_or_unknown() {
    use workdir::{CheckoutSearchResult, GlobResult, GrepResult};
    let glob = CheckoutSearchResult::Glob(GlobResult {
        paths: Vec::new(),
        truncated: false,
    });
    let grep = CheckoutSearchResult::Grep(GrepResult {
        output: String::new(),
        match_count: 0,
        matched_files: 0,
        truncated: false,
        paths: Vec::new(),
    });
    assert_checkout_code(
        crate::search_target::grep_result(glob).err().unwrap(),
        "checkout_unavailable",
    );
    assert_checkout_code(
        crate::search_target::glob_result(grep).err().unwrap(),
        "checkout_unavailable",
    );
    assert_checkout_code(
        crate::file_target::readonly_result_error("Read"),
        "checkout_unavailable",
    );
}

#[test]
fn checkout_relative_path_boundaries() {
    let base = WorkdirPath::new("src").unwrap();
    assert_eq!(
        relative_target(&base, "nested/a.txt", true)
            .unwrap()
            .as_str(),
        "src/nested/a.txt"
    );
    assert_eq!(relative_target(&base, ".", false).unwrap(), base);
    assert_eq!(
        relative_target(&WorkdirPath::root(), "a.txt", true)
            .unwrap()
            .as_str(),
        "a.txt"
    );
    for bad in [
        "",
        "..",
        "../outside",
        "nested/../../outside",
        "/absolute",
        "a\\..\\outside",
    ] {
        assert!(
            relative_target(&base, bad, false).is_err(),
            "accepted {bad}"
        );
    }
    assert!(relative_target(&base, ".", true).is_err());
    assert!(!is_beneath(
        &base,
        &WorkdirPath::new("src-other/a.txt").unwrap()
    ));
    assert!(check_pattern("../*").is_err());
    assert!(check_pattern("/tmp/*").is_err());
    assert!(check_pattern("**/*.rs").is_ok());
}

#[tokio::test]
async fn checkout_read_and_normal_edit_share_hash_and_line_count() {
    let fixture = Fixture::new();
    std::fs::write(fixture.dir.path().join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let read = fixture
        .native("main", "a.txt", "Read", json!({"offset": 1, "limit": 1}))
        .await
        .unwrap();
    assert!(read.output.summary.contains("[2..2] of 3"));
    assert_eq!(read.output.content.as_deref(), Some("     2\tbeta\n"));
    assert!(read.validator.is_some());
    let selected = fixture.router.resolve(Some("main")).unwrap();
    let tracker = fixture
        .tracker
        .scoped_attachment(&selected.alias, selected.generation);
    assert_eq!(
        tracker.observed_workdir_line_count(&WorkdirPath::new("a.txt").unwrap()),
        Some(3)
    );
    fixture.normal("Edit", json!({"target_workdir": "main", "file_path": "a.txt", "old_string": "beta", "new_string": "changed"})).await.unwrap();
    let write = fixture
        .native("main", "a.txt", "Write", json!({"content": "new\n"}))
        .await
        .unwrap();
    assert!(write.output.summary.contains("Overwrote"));
    assert_eq!(fixture.tracker.change_stat().deleted, 4);
    let reread = fixture
        .native("main", "a.txt", "Read", json!({}))
        .await
        .unwrap();
    assert_eq!(reread.output.content.as_deref(), Some("     1\tnew\n"));
}

#[tokio::test]
async fn checkout_normal_read_then_native_edit_uniqueness_and_replace_all() {
    let fixture = Fixture::new();
    std::fs::write(fixture.dir.path().join("a.txt"), "x x x\n").unwrap();
    let before_read = fixture
        .native(
            "main",
            "a.txt",
            "Edit",
            json!({"old_string": "x", "new_string": "y"}),
        )
        .await
        .err()
        .unwrap();
    assert!(before_read.to_string().contains("has not been read"));
    fixture
        .normal(
            "Read",
            json!({"target_workdir": "main", "file_path": "a.txt"}),
        )
        .await
        .unwrap();
    let not_unique = fixture
        .native(
            "main",
            "a.txt",
            "Edit",
            json!({"old_string": "x", "new_string": "y"}),
        )
        .await
        .err()
        .unwrap();
    assert_checkout_code(not_unique, "checkout_invalid");
    assert_eq!(fixture.tracker.change_stat(), crate::ChangeStat::default());
    let edit = fixture
        .native(
            "main",
            "a.txt",
            "Edit",
            json!({"old_string": "x", "new_string": "y", "replace_all": true}),
        )
        .await
        .unwrap();
    assert!(edit.output.summary.contains("3 replacements"));
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("a.txt")).unwrap(),
        "y y y\n"
    );
    fixture.normal("Write", json!({"target_workdir": "main", "file_path": "a.txt", "content": "normal after native"})).await.unwrap();
}

#[tokio::test]
async fn checkout_alias_and_generation_fences() {
    let fixture = Fixture::new();
    fixture.attach("other");
    std::fs::write(fixture.dir.path().join("a.txt"), "same\n").unwrap();
    fixture
        .native("main", "a.txt", "Read", json!({}))
        .await
        .unwrap();
    let wrong_alias = fixture
        .native("other", "a.txt", "Write", json!({"content": "not allowed"}))
        .await
        .err()
        .unwrap();
    assert!(wrong_alias.to_string().contains("has not been read"));
    let (old_generation, old_validator) = fixture.observation("main", "a.txt").await;
    fixture
        .router
        .detach(&WorkdirAttachmentAlias::new("main").unwrap())
        .await
        .unwrap();
    fixture.attach("main");
    let error = execute_checkout_tool(
        fixture.router.clone(),
        fixture.tracker.clone(),
        "main",
        old_generation,
        WorkdirPath::new("a.txt").unwrap(),
        old_validator,
        "Read",
        json!({}),
        Default::default(),
    )
    .await
    .err()
    .unwrap();
    assert!(error.to_string().contains("generation changed"));
    assert_checkout_stale(error);
    let fresh_generation_no_read = fixture
        .native("main", "a.txt", "Write", json!({"content": "not allowed"}))
        .await
        .err()
        .unwrap();
    assert!(
        fresh_generation_no_read
            .to_string()
            .contains("has not been read")
    );
}

#[tokio::test]
async fn checkout_stale_validator_and_hash_are_independent() {
    let fixture = Fixture::new();
    std::fs::write(fixture.dir.path().join("a.txt"), "old\n").unwrap();
    fixture
        .native("main", "a.txt", "Read", json!({}))
        .await
        .unwrap();
    let (generation, validator) = fixture.observation("main", "a.txt").await;
    std::fs::write(fixture.dir.path().join("a.txt"), "externally changed\n").unwrap();
    let stale = execute_checkout_tool(
        fixture.router.clone(),
        fixture.tracker.clone(),
        "main",
        generation,
        WorkdirPath::new("a.txt").unwrap(),
        validator,
        "Write",
        json!({"content": "bad"}),
        Default::default(),
    )
    .await
    .err()
    .unwrap();
    assert_checkout_stale(stale);
    let hash = fixture
        .native("main", "a.txt", "Write", json!({"content": "bad"}))
        .await
        .err()
        .unwrap();
    assert_checkout_stale(hash);
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("a.txt")).unwrap(),
        "externally changed\n"
    );
    assert_eq!(fixture.tracker.change_stat(), crate::ChangeStat::default());
}

#[tokio::test]
async fn checkout_create_new_missing_parents_never_overwrites() {
    let fixture = Fixture::new();
    let created = fixture
        .native(
            "main",
            ".",
            "Create",
            json!({"path": "nested/deep/a.txt", "content": "new\n"}),
        )
        .await
        .unwrap();
    assert!(created.output.summary.contains("Created"));
    assert_eq!(
        created.paths,
        vec![WorkdirPath::new("nested/deep/a.txt").unwrap()]
    );
    let duplicate = fixture
        .native(
            "main",
            ".",
            "Create",
            json!({"path": "nested/deep/a.txt", "content": "overwrite"}),
        )
        .await;
    assert!(duplicate.is_err());
    assert_eq!(
        std::fs::read_to_string(fixture.dir.path().join("nested/deep/a.txt")).unwrap(),
        "new\n"
    );
    // Successful creation updates the same tracker as normal Tool Write.
    fixture.normal("Write", json!({"target_workdir": "main", "file_path": "nested/deep/a.txt", "content": "changed"})).await.unwrap();
}

#[tokio::test]
async fn checkout_rejects_argument_overrides_and_escapes() {
    let fixture = Fixture::new();
    std::fs::write(fixture.dir.path().join("a.txt"), "safe").unwrap();
    for (name, arguments) in [
        ("Read", json!({"file_path": "other"})),
        ("Read", json!({"target_workdir": "other"})),
        ("Write", json!({"content": "bad", "validator": []})),
        ("Create", json!({"path": "../outside", "content": "bad"})),
        ("Glob", json!({"pattern": "*", "path": "/absolute"})),
        ("Grep", json!({"pattern": "safe", "path": "../outside"})),
        ("Other", json!({})),
    ] {
        let path = if ["Create", "Glob", "Grep"].contains(&name) {
            "."
        } else {
            "a.txt"
        };
        let error = fixture
            .native("main", path, name, arguments)
            .await
            .err()
            .unwrap();
        assert!(
            matches!(error, ToolError::InvalidArgument(_)),
            "{name}: {error}"
        );
    }
}

#[tokio::test]
async fn checkout_search_typed_paths_and_normal_rendering() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.dir.path().join("src/nested")).unwrap();
    std::fs::write(
        fixture.dir.path().join("src/nested/a.txt"),
        "context\nhit\nafter\n",
    )
    .unwrap();
    std::fs::write(fixture.dir.path().join("outside.txt"), "hit\n").unwrap();
    let glob = fixture
        .native(
            "main",
            "src",
            "Glob",
            json!({"pattern": "**/*.txt", "path": "nested"}),
        )
        .await
        .unwrap();
    assert_eq!(
        glob.paths,
        vec![WorkdirPath::new("src/nested/a.txt").unwrap()]
    );
    let normal = fixture
        .normal(
            "Glob",
            json!({"target_workdir": "main", "pattern": "**/*.txt", "path": "src/nested"}),
        )
        .await
        .unwrap();
    assert_eq!(glob.output.summary, normal.summary);
    assert_eq!(glob.output.content, normal.content);
    for mode in ["files_with_matches", "content", "count"] {
        let grep = fixture
            .native(
                "main",
                "src",
                "Grep",
                json!({"pattern": "hit", "output_mode": mode, "-C": 1}),
            )
            .await
            .unwrap();
        assert_eq!(
            grep.paths,
            vec![WorkdirPath::new("src/nested/a.txt").unwrap()]
        );
        let normal = fixture.normal("Grep", json!({"target_workdir": "main", "path": "src", "pattern": "hit", "output_mode": mode, "-C": 1})).await.unwrap();
        assert_eq!(grep.output.summary, normal.summary);
        assert_eq!(grep.output.content, normal.content);
        assert!(grep.listing.is_none());
        assert!(!grep.output.content.unwrap().contains("outside.txt"));
    }
}

#[tokio::test]
async fn checkout_list_returns_one_typed_page_and_provider_coordinate_continuation() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(fixture.dir.path().join("src/nested")).unwrap();
    std::fs::write(fixture.dir.path().join("src/a.txt"), "alpha").unwrap();
    std::fs::write(fixture.dir.path().join("src/z.txt"), "z").unwrap();
    std::fs::write(fixture.dir.path().join("outside.txt"), "outside").unwrap();
    let (_, validator) = fixture.observation("main", "src").await;
    let first = fixture
        .native("main", "src", "List", json!({"limit": 1}))
        .await
        .unwrap();
    assert!(first.paths.is_empty());
    assert!(first.output.content.is_none());
    assert!(first.output.summary.contains("Listed 1 of 3 entries"));
    assert!(first.output.summary.contains("truncated"));
    assert_eq!(first.validator, Some(validator));
    let page = first.listing.unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].path.as_str(), "src/nested");
    assert_eq!(page.entries[0].kind, workdir::EntryKind::Directory);
    assert!(page.truncated);
    let total_bytes = page.total_bytes;
    assert!(total_bytes >= 6);
    let cursor = page.next_after.unwrap();
    assert_eq!(cursor.path.as_str(), "src/nested");
    let second = fixture
        .native("main", "src", "List", json!({"after": cursor}))
        .await
        .unwrap();
    let page = second.listing.unwrap();
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["src/a.txt", "src/z.txt"]
    );
    assert_eq!(page.total_entries, 3);
    assert_eq!(page.total_bytes, total_bytes);
    assert!(!page.truncated);
    assert!(page.next_after.is_none());
    assert_eq!(fixture.tracker.change_stat(), crate::ChangeStat::default());
}

#[tokio::test]
async fn checkout_list_root_file_cursor_stays_on_selected_attachment() {
    let fixture = Fixture::new();
    std::fs::write(fixture.dir.path().join("a.txt"), "a").unwrap();
    std::fs::write(fixture.dir.path().join("z.txt"), "z").unwrap();
    let other = TempDir::new().unwrap();
    std::fs::write(other.path().join("wrong.txt"), "wrong").unwrap();
    fixture
        .router
        .attach(
            WorkdirAttachmentAlias::new("other").unwrap(),
            Arc::new(LocalWorkdirSession::new(
                Scope::writable(other.path()).unwrap(),
                other.path().to_path_buf(),
            )),
        )
        .unwrap();
    let first = fixture
        .native("main", ".", "List", json!({"limit": 1}))
        .await
        .unwrap()
        .listing
        .unwrap();
    let cursor = first.next_after.unwrap();
    assert_eq!(cursor.kind, workdir::EntryKind::File);
    assert_eq!(cursor.path.as_str(), "a.txt");
    let second = fixture
        .native("main", ".", "List", json!({"after": cursor}))
        .await
        .unwrap()
        .listing
        .unwrap();
    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.entries[0].path.as_str(), "z.txt");
    let other_page = fixture
        .native("other", ".", "List", json!({}))
        .await
        .unwrap()
        .listing
        .unwrap();
    assert_eq!(other_page.entries.len(), 1);
    assert_eq!(other_page.entries[0].path.as_str(), "wrong.txt");
}

#[tokio::test]
async fn checkout_list_defaults_to_100_and_accepts_both_limit_bounds() {
    let fixture = Fixture::new();
    for index in 0..101 {
        std::fs::write(fixture.dir.path().join(format!("{index:03}.txt")), "").unwrap();
    }
    for (arguments, count, truncated) in [
        (json!({}), 100, true),
        (json!({"limit": 1}), 1, true),
        (json!({"limit": 1000}), 101, false),
    ] {
        let result = fixture
            .native("main", ".", "List", arguments)
            .await
            .unwrap();
        let page = result.listing.unwrap();
        assert_eq!(page.entries.len(), count);
        assert_eq!(page.truncated, truncated);
        assert_eq!(page.next_after.is_some(), truncated);
    }
}

#[tokio::test]
async fn checkout_list_rejects_route_overrides_malformed_limits_and_nonchild_cursors() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.dir.path().join("src")).unwrap();
    for arguments in [
        json!({"path": "."}),
        json!({"target_workdir": "other"}),
        json!({"generation": 1}),
        json!({"validator": []}),
        json!({"limit": 0}),
        json!({"limit": 1001}),
        json!({"limit": -1}),
        json!({"limit": 1.5}),
        json!({"limit": "1"}),
        json!({"limit": null}),
        json!({"after": "src/a.txt"}),
        json!({"after": {"path": "src/a.txt"}}),
        json!({"after": {"kind": "file", "path": "src/a.txt", "extra": 1}}),
        json!({"after": {"kind": "bogus", "path": "src/a.txt"}}),
        json!({"after": {"kind": "file", "path": "src"}}),
        json!({"after": {"kind": "directory", "path": "src/nested/deep"}}),
        json!({"after": {"kind": "file", "path": "src-other/a.txt"}}),
        json!({"after": {"kind": "file", "path": "a.txt"}}),
        json!({"after": {"kind": "file", "path": "/src/a.txt"}}),
        json!({"after": {"kind": "file", "path": "src/../a.txt"}}),
        json!({"after": {"kind": "file", "path": "."}}),
    ] {
        let error = fixture
            .native("main", "src", "List", arguments.clone())
            .await
            .err()
            .unwrap();
        assert!(
            matches!(error, ToolError::InvalidArgument(_)),
            "{arguments}: {error}"
        );
    }
}

#[tokio::test]
async fn checkout_list_requires_current_generation_and_directory_validator() {
    let fixture = Fixture::new();
    let (generation, validator) = fixture.observation("main", ".").await;
    // No timing dependency: make the supplied observation explicitly stale.
    let mut stale_validator = validator.clone();
    stale_validator.push(0);
    for (generation, validator) in [(generation, stale_validator), (generation + 1, validator)] {
        let error = execute_checkout_tool(
            fixture.router.clone(),
            fixture.tracker.clone(),
            "main",
            generation,
            WorkdirPath::root(),
            validator,
            "List",
            json!({}),
            Default::default(),
        )
        .await
        .err()
        .unwrap();
        assert_checkout_stale(error);
    }
}
