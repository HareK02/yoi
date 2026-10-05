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
        assert!(!grep.output.content.unwrap().contains("outside.txt"));
    }
}
