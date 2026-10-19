//! Native Bash output integration through real WIP Client/Host codec and local process pipes.
//! No telemetry/storage fake: only the model client is absent. Runtime HTTP transport is tested
//! by worker-runtime's command_transport_tests. Each fixture owns its Workdir and ephemeral view.
use super::*;
use crate::file_operation::Contract;
use tempfile::TempDir;
use tools::bash_output::BashOutputView;
use workdir::{
    LocalWorkdirSession, WorkdirAttachmentAlias, WorkdirSessionHandle, WorkdirSessionRouter,
};

struct Fixture {
    _dir: TempDir,
    router: Arc<WorkdirSessionRouter>,
    view: BashOutputView,
    runtime: WipRuntime,
}
impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("real.txt"), "first\n雪 needle\nlast\n").unwrap();
        let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::new(
            manifest::Scope::writable(dir.path()).unwrap(),
            dir.path().into(),
        ));
        let router = Arc::new(WorkdirSessionRouter::new());
        router
            .attach(WorkdirAttachmentAlias::new("main").unwrap(), session)
            .unwrap();
        let view = BashOutputView::new();
        let runtime = runtime(router.clone(), view.clone());
        Self {
            _dir: dir,
            router,
            view,
            runtime,
        }
    }
    async fn run(&self, command: &str) -> Json {
        value(
            self.runtime
                .invoke(
                    "/bash-output".into(),
                    contextual_reference("yoi.bash/command/v1", "/bash-output"),
                    "run".into(),
                    json!({"input":{"command":command,"cwd":"."}}),
                    Default::default(),
                )
                .await
                .unwrap(),
        )
    }
}
fn runtime(router: Arc<WorkdirSessionRouter>, view: BashOutputView) -> WipRuntime {
    let mut registry = WipMountRegistry::new();
    crate::checkout::mount_checkouts(&mut registry, router.clone(), tools::Tracker::new(), None)
        .unwrap();
    crate::feature::builtin::bash::mount(&mut registry, router, view, None).unwrap();
    WipRuntime::new(WipHost::new(registry), SecurityContext::new("bash-test"), 0).unwrap()
}
fn value(output: ToolOutput) -> Json {
    serde_json::from_str(&output.content.unwrap()).unwrap()
}
async fn invoke(
    runtime: &WipRuntime,
    path: &str,
    contract: Contract,
    operation: &str,
    args: Json,
) -> Result<Json, ToolError> {
    runtime
        .invoke(
            path.into(),
            contract.reference(),
            operation.into(),
            args,
            Default::default(),
        )
        .await
        .map(value)
}

#[tokio::test]
async fn native_bash_entries_reuse_checkout_contract_for_direct_inspect_read_search() {
    let f = Fixture::new();
    let result = f
        .run("printf 'first\n雪 needle\nlast\n'; printf error >&2; exit 3")
        .await;
    assert_eq!(result["metadata"]["terminal"]["exit_code"], 3);
    let stdout = result["stdout_entry"].as_str().unwrap();
    assert!(stdout.starts_with("/bash-output/"));
    // The native result Descriptor gives these coordinates TypeExpr::Entry; Object inspection
    // contains capabilities and metadata, not the process output body.
    let projection = f
        .runtime
        .host
        .projection_live("/bash-output")
        .await
        .unwrap()
        .unwrap();
    let TypeExpr::Record { fields } = &projection.descriptor.operations[0].returns.r#type else {
        panic!("native record");
    };
    assert_eq!(
        fields
            .iter()
            .find(|v| v.name == "stdout_entry")
            .unwrap()
            .r#type,
        TypeExpr::Entry
    );
    let inspect = f.runtime.inspect(stdout.into(), false).await.unwrap();
    assert!(!inspect.content.unwrap().contains("雪 needle"));
    for path in ["/checkouts/main/real.txt", stdout] {
        let read = invoke(
            &f.runtime,
            path,
            Contract::ReadLines,
            "read",
            json!({"offset":1,"limit":1}),
        )
        .await
        .unwrap();
        assert!(read["content"].as_str().unwrap().contains("雪 needle"));
        assert_eq!(read["next_offset"], 2);
        let search = invoke(
            &f.runtime,
            path,
            Contract::SearchText,
            "search",
            json!({"offset":0,"limit":2,"pattern":"needle"}),
        )
        .await
        .unwrap();
        assert_eq!(search["matches"][0]["line"], 2);
        assert_eq!(search["next_offset"], 2);
        assert_eq!(search["truncated"], true);
    }
    for name in ["stdout_entry", "stderr_entry", "order_entry"] {
        assert_eq!(
            fields.iter().find(|v| v.name == name).unwrap().r#type,
            TypeExpr::Entry
        );
        f.runtime
            .inspect(result[name].as_str().unwrap().into(), false)
            .await
            .unwrap();
    }
    let stderr = invoke(
        &f.runtime,
        result["stderr_entry"].as_str().unwrap(),
        Contract::ReadLines,
        "read",
        json!({}),
    )
    .await
    .unwrap();
    assert!(stderr["content"].as_str().unwrap().contains("error"));
    let order = invoke(
        &f.runtime,
        result["order_entry"].as_str().unwrap(),
        Contract::ReadLines,
        "read",
        json!({}),
    )
    .await
    .unwrap();
    assert!(order["content"].as_str().unwrap().contains("Stdout"));
    assert!(order["content"].as_str().unwrap().contains("Stderr"));
    assert_eq!(result["metadata"]["ordering"], "provider_read_observation");
    let tree = value(
        f.runtime
            .tree("/bash-output".into(), 8, true)
            .await
            .unwrap(),
    );
    assert!(!tree.to_string().contains(stdout));
    assert!(
        invoke(
            &f.runtime,
            stdout,
            Contract::Write,
            "write",
            json!({"content":"no"})
        )
        .await
        .is_err()
    );
    assert!(
        invoke(
            &f.runtime,
            stdout,
            Contract::Edit,
            "edit",
            json!({"old_string":"first","new_string":"no"})
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn common_byte_reader_handles_long_lines_raw_invalid_utf8_boundaries_and_eof() {
    let f = Fixture::new();
    let result = f.run("printf '%100000s' ''; printf '€\\377'").await;
    let path = result["stdout_entry"].as_str().unwrap();
    assert!(
        invoke(
            &f.runtime,
            path,
            Contract::ReadLines,
            "read",
            json!({"limit":1})
        )
        .await
        .is_err()
    );
    for (offset, expected) in [
        (99999, vec![b' ', 0xe2]),
        (100001, vec![0x82, 0xac]),
        (100003, vec![0xff]),
    ] {
        let read = invoke(
            &f.runtime,
            path,
            Contract::ReadBytes,
            "read",
            json!({"offset":offset,"limit":2}),
        )
        .await
        .unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(read["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes, expected);
        assert_eq!(read["next_offset"], offset + expected.len() as u64);
    }
    let eof = invoke(
        &f.runtime,
        path,
        Contract::ReadBytes,
        "read",
        json!({"offset":100004,"limit":2}),
    )
    .await
    .unwrap();
    assert_eq!(eof["data"], "");
    assert_eq!(eof["eof"], true);
    assert!(
        invoke(
            &f.runtime,
            path,
            Contract::ReadBytes,
            "read",
            json!({"offset":100005})
        )
        .await
        .is_err()
    );
    assert!(
        invoke(
            &f.runtime,
            path,
            Contract::ReadBytes,
            "read",
            json!({"limit":32769})
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn publication_and_invoke_recheck_child_authority_and_root_teardown_invalidates_old_refs() {
    let f = Fixture::new();
    let lease = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let child = f.view.child({
        let lease = lease.clone();
        Arc::new(move || lease.load(std::sync::atomic::Ordering::Acquire))
    });
    let child_runtime = runtime(f.router.clone(), child.clone());
    let result = value(
        child_runtime
            .invoke(
                "/bash-output".into(),
                contextual_reference("yoi.bash/command/v1", "/bash-output"),
                "run".into(),
                json!({"input":{"command":"printf child-evidence","cwd":"."}}),
                Default::default(),
            )
            .await
            .unwrap(),
    );
    let path = result["stdout_entry"].as_str().unwrap();
    child_runtime.inspect(path.into(), false).await.unwrap();
    let sibling = runtime(f.router.clone(), f.view.child(Arc::new(|| true)));
    let unrelated = runtime(f.router.clone(), BashOutputView::new());
    assert!(sibling.inspect(path.into(), false).await.is_err());
    assert!(unrelated.inspect(path.into(), false).await.is_err());
    lease.store(false, std::sync::atomic::Ordering::Release);
    assert!(
        invoke(&child_runtime, path, Contract::ReadBytes, "read", json!({}))
            .await
            .is_err()
    );
    assert!(
        invoke(
            &child_runtime,
            path,
            Contract::SearchText,
            "search",
            json!({"pattern":"child"})
        )
        .await
        .is_err()
    );
    assert!(
        invoke(&f.runtime, path, Contract::ReadBytes, "read", json!({}))
            .await
            .is_ok()
    );
    f.view.revoke();
    assert!(
        invoke(&f.runtime, path, Contract::ReadBytes, "read", json!({}))
            .await
            .is_err()
    );
    let restored = runtime(f.router.clone(), BashOutputView::new());
    assert!(restored.inspect(path.into(), false).await.is_err());
}

#[tokio::test]
async fn native_timeout_and_cancellation_keep_terminal_evidence_instead_of_success() {
    let f = Fixture::new();
    let session = f.router.only_session().unwrap();
    let mut events = session.subscribe_command_events().unwrap();
    let runtime = Arc::new(f.runtime);
    let context = agen::tool::ToolExecutionContext::new("native-cancel", "native-attempt", 0);
    let running = runtime.clone();
    let executing_context = context.clone();
    let execution = tokio::spawn(async move {
        running
            .invoke(
                "/bash-output".into(),
                contextual_reference("yoi.bash/command/v1", "/bash-output"),
                "run".into(),
                json!({"input":{"command":"printf before; while :; do :; done","cwd":"."}}),
                executing_context,
            )
            .await
    });
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
    .unwrap();
    runtime.cancel(&context).await.unwrap();
    let ToolError::Cancelled(output) =
        tokio::time::timeout(std::time::Duration::from_secs(5), execution)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
    else {
        panic!("expected cancelled terminal output")
    };
    let body: Json = serde_json::from_str(&output.content.unwrap()).unwrap();
    let id = body["output"]["id"].as_str().unwrap();
    assert_eq!(body["output"]["terminal"]["transport_complete"], true);
    assert_eq!(body["output"]["terminal"]["status"], "cancelled");
    assert_eq!(
        f.view
            .read(id, workdir::CommandStream::Stdout, 0, 20)
            .unwrap(),
        b"before"
    );
    assert!(
        invoke(
            &runtime,
            body["stdout_entry"].as_str().unwrap(),
            Contract::ReadBytes,
            "read",
            json!({})
        )
        .await
        .is_ok()
    );
    let error = runtime.invoke("/bash-output".into(), contextual_reference("yoi.bash/command/v1", "/bash-output"), "run".into(), json!({"input":{"command":"printf timeout; while :; do :; done","cwd":".","timeout":1}}), Default::default()).await.unwrap_err();
    let ToolError::Interrupted(output) = error else {
        panic!("expected timeout evidence")
    };
    let body: Json = serde_json::from_str(&output.content.unwrap()).unwrap();
    assert_eq!(body["output"]["terminal"]["status"], "timed_out");
    assert_eq!(body["output"]["terminal"]["transport_complete"], true);
    assert!(
        f.view
            .metadata(body["output"]["id"].as_str().unwrap())
            .is_some()
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn common_read_and_search_permissions_are_independent_from_bash_execution() {
    let f = Fixture::new();
    let result = f.run("printf needle").await;
    let path = result["stdout_entry"].as_str().unwrap();
    let mut registry = WipMountRegistry::new();
    crate::feature::builtin::bash::mount(
        &mut registry,
        f.router.clone(),
        f.view.clone(),
        Some(ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: vec![manifest::ToolPermissionRule {
                tool: "Grep".into(),
                pattern: "*".into(),
                action: ToolPermissionAction::Allow,
            }],
        }),
    )
    .unwrap();
    let restricted = WipRuntime::new(
        WipHost::new(registry),
        SecurityContext::new("restricted"),
        0,
    )
    .unwrap();
    restricted.inspect(path.into(), false).await.unwrap();
    assert!(
        invoke(&restricted, path, Contract::ReadBytes, "read", json!({}))
            .await
            .is_err()
    );
    assert!(
        invoke(&restricted, path, Contract::ReadLines, "read", json!({}))
            .await
            .is_err()
    );
    let found = invoke(
        &restricted,
        path,
        Contract::SearchText,
        "search",
        json!({"pattern":"needle"}),
    )
    .await
    .unwrap();
    assert!(found["matches"].as_array().unwrap().len() == 1);
    assert!(
        restricted
            .invoke(
                "/bash-output".into(),
                contextual_reference("yoi.bash/command/v1", "/bash-output"),
                "run".into(),
                json!({"input":{"command":"touch denied","cwd":"."}}),
                Default::default()
            )
            .await
            .is_err()
    );
    assert!(!f._dir.path().join("denied").exists());
}

#[tokio::test]
async fn multibyte_long_line_search_refuses_instead_of_skipping_an_unread_suffix() {
    let f = Fixture::new();
    let result = f
        .run("printf '雪%.0s' {1..22000}; printf 'needle\nnext\n'")
        .await;
    let path = result["stdout_entry"].as_str().unwrap();
    let error = invoke(
        &f.runtime,
        path,
        Contract::SearchText,
        "search",
        json!({"pattern":"needle","offset":0,"limit":1}),
    )
    .await
    .unwrap_err();
    assert!(
        error.to_string().contains("ResourceLimitExceeded"),
        "{error}"
    );
    // No successful page claims to advance past a partial line. Raw bytes can still
    // retrieve its unread suffix using the same common file contract.
    let suffix = invoke(
        &f.runtime,
        path,
        Contract::ReadBytes,
        "read",
        json!({"offset":66000,"limit":7}),
    )
    .await
    .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(suffix["data"].as_str().unwrap())
            .unwrap(),
        b"needle\n"
    );
    assert_eq!(suffix["next_offset"], 66007);
}
