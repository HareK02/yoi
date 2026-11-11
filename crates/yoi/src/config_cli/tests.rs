//! CLI retry/file protection tests with scripted HTTP peers, NOT Server adoption tests.
//! The real catalog/authentication/SQLite connection is covered in workspace-server.
use super::*;
use server_api::{
    ConfigurationPreviewResponse, RepositoryApiError, RuntimePolicyApplication,
    client_support::NoAuthorizer,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn base() -> ConfigurationSourceResponse {
    serde_json::from_value(serde_json::json!({
        "state": {"snapshot":{"digest":"root-a","entries":{"main.dcdl":{"path":"main.dcdl","content_type":"decodal","content":"{ value = 1; }\n","content_digest":"file-a"}, "notes.txt":{"path":"notes.txt","content_type":"text","content":"note","content_digest":"note-a"}}}, "contract":{"contract_version":2,"decodal_version":"0.4.0","schema_version":1,"entrypoints":["main.dcdl"],"import_policy_version":1,"schema_bundle":{"contributions":[],"source":"","fingerprint":"schema"},"fingerprint":"toolchain"},"projection_digest":"projection-a"},
        "history_head":"head-a", "source_stored":true,"catalog":{"workspace":"workspace-test","namespaces":[]},"catalog_digest":"catalog-a","policies":[]
    })).unwrap()
}
fn checkout() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("checkout");
    install(&dir, "http://backend.test", "workspace-test", base()).unwrap();
    (temp, dir)
}
fn loaded(dir: &Path) -> Checkout {
    read_json(&dir.join(".yoi-config/state.json")).unwrap()
}
fn options(dir: &Path, command: Command) -> Options {
    Options {
        directory: dir.to_owned(),
        command,
    }
}
fn preview() -> String {
    serde_json::to_string(&ConfigurationPreviewResponse {
        expected_head: Some("head-a".into()),
        state: base().state,
        changed_paths: vec!["main.dcdl".into()],
        changed_subjects: vec![],
    })
    .unwrap()
}
fn receipt(request: &ConfigurationAdoptionRequest) -> ConfigurationAdoptionReceipt {
    let mut state = base().state;
    state.snapshot.digest = "root-b".into();
    let entry = state.snapshot.entries.get_mut("main.dcdl").unwrap();
    entry.content = "{ value = 2; }\n".into();
    entry.content_digest = "file-b".into();
    ConfigurationAdoptionReceipt {
        workspace_id: "workspace-test".into(),
        operation_id: request.operation_id.clone(),
        history_head: "head-b".into(),
        state,
        source_stored: true,
        policy_committed: true,
        runtime_application: RuntimePolicyApplication::NextCurrentPolicyCheck,
    }
}

#[test]
fn delta_omits_unchanged_files_and_marks_add_update_delete_explicitly() {
    let (_temp, dir) = checkout();
    let checkout = LockedCheckout::open(&dir, "http://backend.test", "workspace-test").unwrap();
    assert!(checkout.request().unwrap().source.changes.is_empty());
    fs::write(dir.join("source/main.dcdl"), "{ value = 2; }\n").unwrap();
    fs::remove_file(dir.join("source/notes.txt")).unwrap();
    fs::write(dir.join("source/extra.dcdl"), "{}\n").unwrap();
    let request = checkout.request().unwrap();
    assert_eq!(request.expected_head.as_deref(), Some("head-a"));
    assert_eq!(request.source.base_digest, "root-a");
    assert!(
        matches!(&request.source.changes[0], ConfigTreeChange::Create { path, content_type: ConfigContentType::Decodal, .. } if path == "extra.dcdl")
    );
    assert!(
        matches!(&request.source.changes[1], ConfigTreeChange::Update { path, expected_digest, .. } if path == "main.dcdl" && expected_digest == "file-a")
    );
    assert!(
        matches!(&request.source.changes[2], ConfigTreeChange::Delete { path, expected_digest } if path == "notes.txt" && expected_digest == "note-a")
    );
    assert_eq!(request.source.entrypoints, vec!["main.dcdl"]);
}

#[test]
fn checkout_lock_and_workspace_identity_prevent_concurrent_or_misdirected_commands() {
    let (_temp, dir) = checkout();
    let lock = LockedCheckout::open(&dir, "http://backend.test", "workspace-test").unwrap();
    assert!(LockedCheckout::open(&dir, "http://backend.test", "workspace-test").is_err());
    drop(lock);
    assert!(LockedCheckout::open(&dir, "http://other.test", "workspace-test").is_err());
    assert!(LockedCheckout::open(&dir, "http://backend.test", "other-workspace").is_err());
}

#[test]
fn path_escape_and_symlink_source_are_rejected_without_reading_targets() {
    for path in [
        "/absolute",
        "../outside",
        "a/../b",
        "a//b",
        "a\\b",
        "./a",
        "",
    ] {
        assert!(safe_relative(path).is_err(), "{path}");
    }
    #[cfg(unix)]
    {
        let (temp, dir) = checkout();
        let secret = temp.path().join("secret");
        fs::write(&secret, "secret-body").unwrap();
        std::os::unix::fs::symlink(secret, dir.join("source/escape")).unwrap();
        let error = LockedCheckout::open(&dir, "http://backend.test", "workspace-test")
            .unwrap()
            .request()
            .unwrap_err();
        assert!(!error.to_string().contains("secret-body"));
    }
}

#[test]
fn malformed_manifest_and_type_change_are_local_errors() {
    let (_temp, dir) = checkout();
    let checkout = LockedCheckout::open(&dir, "http://backend.test", "workspace-test").unwrap();
    let mut edit = edit_for(&base());
    edit.content_types
        .insert("main.dcdl".into(), ConfigContentType::Text);
    atomic_json(&dir.join("edit.json"), &edit).unwrap();
    assert_eq!(checkout.request().unwrap_err().exit_code, 2);
    fs::write(dir.join("edit.json"), "{").unwrap();
    assert_eq!(checkout.request().unwrap_err().exit_code, 2);
}

#[test]
fn parser_requires_explicit_command_directory_and_no_ignored_options() {
    let args = |values: &[&str]| values.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        parse(&args(&["pull", "--dir", "checkout"]))
            .unwrap()
            .unwrap()
            .command,
        Command::Pull
    );
    for values in [
        vec!["push"],
        vec!["pull", "--dir", ""],
        vec!["push", "--dir=x", "--force"],
        vec!["bogus", "--dir=x"],
    ] {
        assert!(parse(&args(&values)).is_err());
    }
}

struct Script {
    origin: String,
    received: Arc<Mutex<Vec<(String, String)>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Script {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Script {
    async fn new(responses: Vec<Option<String>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Vec::new()));
        let capture = received.clone();
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (end, length) = loop {
                    let mut chunk = [0u8; 4096];
                    let n = stream.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                    assert!(bytes.len() < 1024 * 1024);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while bytes.len() < end + length {
                    let mut chunk = [0u8; 4096];
                    let n = stream.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let first = String::from_utf8_lossy(&bytes[..end])
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned();
                let body = String::from_utf8(bytes[end..end + length].to_vec()).unwrap();
                capture.lock().unwrap().push((first, body));
                if let Some(response) = response {
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
                // None drops the connection after receiving the complete request.
            }
        });
        Self {
            origin,
            received,
            task,
        }
    }
    fn api(&self) -> ConfigurationAdoptionApiClient<NoAuthorizer> {
        ConfigurationAdoptionApiClient::builder(&self.origin)
            .unwrap()
            .request_timeout(Duration::from_secs(3))
            .build()
            .unwrap()
    }
}
fn http(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
fn configuration_problem(
    operation: server_api::configuration_failure::ConfigurationFailureOperation,
    reason: server_api::configuration_failure::ConfigurationFailureReason,
) -> serde_json::Value {
    use server_api::configuration_failure::{
        ConfigurationFailureDetails, ConfigurationFailureVisibility,
    };
    let facts =
        error_contract::FailureFacts::new(ConfigurationFailureDetails { operation, reason })
            .unwrap();
    let projected = error_contract::project_failure::<error_contract::Browser, _>(
        &facts,
        &ConfigurationFailureVisibility {
            can_view_cause: true,
        },
    )
    .unwrap();
    let mut problem = serde_json::to_value(ConfigurationApiError::projected(projected)).unwrap();
    problem["detail"] = "raw provider /private/config token=do-not-expose".into();
    problem["yoi_failure"]["explanation"] =
        "raw provider /private/config token=do-not-expose".into();
    problem
}
fn problem_http(status: u16, problem: &serde_json::Value) -> String {
    http(status, &problem.to_string()).replacen(
        "Content-Type: application/json",
        "Content-Type: application/problem+json",
        1,
    )
}

#[tokio::test]
async fn typed_configuration_refusal_explains_cause_without_settling_an_earlier_request() {
    use server_api::configuration_failure::{
        ConfigurationFailureOperation as Op, ConfigurationFailureReason as Reason,
    };
    for (reason, status, exit) in [
        (Reason::InvalidImport, 400, 3),
        (Reason::EditingHeadChanged, 409, 4),
        (Reason::OwnerRequired, 403, 5),
    ] {
        for retry in [false, true] {
            let (_temp, dir) = checkout();
            let pending = if retry {
                let mut checkout =
                    LockedCheckout::open(&dir, "http://backend.test", "workspace-test").unwrap();
                let request = checkout.request().unwrap();
                checkout.state.pending = Some(request.clone());
                checkout.save().unwrap();
                Some(request)
            } else {
                None
            };
            let mut responses = Vec::new();
            if !retry {
                responses.push(Some(http(200, &preview())));
            }
            responses.push(Some(problem_http(
                status,
                &configuration_problem(Op::Adopt, reason),
            )));
            if retry {
                responses.push(Some(http(200, r#"{"status":"not_observed"}"#)));
            }
            let server = Script::new(responses).await;
            let error = invoke(
                &server.api(),
                &dir,
                if retry { Command::Retry } else { Command::Push },
            )
            .await
            .unwrap_err();
            assert_eq!(error.exit_code, exit);
            assert!(
                error.message.contains(reason.explanation()),
                "{}",
                error.message
            );
            assert!(error.message.contains(Op::Adopt.description()));
            assert!(!error.message.contains("raw provider"));
            assert!(!error.message.contains("/private/"));
            assert!(!error.message.contains("do-not-expose"));
            assert_eq!(loaded(&dir).pending, pending);
            if retry {
                assert!(error.message.contains("earlier outcome still unresolved"));
                let unconfirmed = invoke(&server.api(), &dir, Command::Reconcile)
                    .await
                    .unwrap_err();
                assert_eq!(unconfirmed.exit_code, 7);
                assert!(unconfirmed.message.contains("NotObserved"));
                assert_eq!(loaded(&dir).pending, pending);
                let requests = server.received.lock().unwrap();
                for (_, body) in requests.iter() {
                    assert_eq!(
                        serde_json::from_str::<ConfigurationAdoptionRequest>(body).unwrap(),
                        pending.clone().unwrap()
                    );
                }
            }
            assert!(dir.join("source/main.dcdl").exists());
        }
    }
}

#[tokio::test]
async fn unsupported_or_corrupt_configuration_problem_keeps_original_commit_unknown() {
    use server_api::configuration_failure::{
        ConfigurationFailureOperation as Op, ConfigurationFailureReason as Reason,
    };
    for future in [false, true] {
        let (_temp, dir) = checkout();
        let mut problem = configuration_problem(Op::Adopt, Reason::EditingHeadChanged);
        if future {
            problem["yoi_failure"]["details_version"] = 999.into();
        } else {
            problem["yoi_failure"]["details"]["reason"] = "owner_required".into();
        }
        let server = Script::new(vec![
            Some(http(200, &preview())),
            Some(problem_http(409, &problem)),
        ])
        .await;
        let error = invoke(&server.api(), &dir, Command::Push)
            .await
            .unwrap_err();
        assert_eq!(error.exit_code, 7);
        assert!(!error.message.contains("raw provider"));
        let requests = server.received.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            serde_json::from_str::<ConfigurationAdoptionRequest>(&requests[1].1).unwrap(),
            loaded(&dir).pending.unwrap()
        );
    }
}

async fn invoke(
    api: &ConfigurationAdoptionApiClient<NoAuthorizer>,
    dir: &Path,
    command: Command,
) -> Result<String, Error> {
    tokio::time::timeout(
        Duration::from_secs(5),
        run(
            api,
            "http://backend.test",
            "workspace-test",
            &options(dir, command),
        ),
    )
    .await
    .expect("bounded CLI operation")
}

#[tokio::test]
async fn lost_commit_response_retains_exact_payload_and_later_edits_through_retry() {
    let (_temp, dir) = checkout();
    fs::write(dir.join("source/main.dcdl"), "{ value = 2; }\n").unwrap();
    let server = Script::new(vec![Some(http(200, &preview())), None]).await;
    let error = invoke(&server.api(), &dir, Command::Push)
        .await
        .unwrap_err();
    assert_eq!(error.exit_code, 7);
    let pending = loaded(&dir).pending.unwrap();
    let requests = server.received.lock().unwrap();
    assert!(requests[1].0.contains("/commit"));
    assert_eq!(
        serde_json::from_str::<ConfigurationAdoptionRequest>(&requests[1].1).unwrap(),
        pending
    );
    drop(requests);
    fs::write(dir.join("source/main.dcdl"), "{ value = 3; }\n").unwrap();
    assert_eq!(
        invoke(&server.api(), &dir, Command::Push)
            .await
            .unwrap_err()
            .exit_code,
        7
    );
    let result = receipt(&pending);
    let retry = Script::new(vec![Some(http(
        200,
        &serde_json::to_string(&result).unwrap(),
    ))])
    .await;
    assert!(
        invoke(&retry.api(), &dir, Command::Retry)
            .await
            .unwrap()
            .contains("adopted")
    );
    assert_eq!(
        serde_json::from_str::<ConfigurationAdoptionRequest>(&retry.received.lock().unwrap()[0].1)
            .unwrap(),
        pending
    );
    assert!(loaded(&dir).pending.is_none());
    assert_eq!(loaded(&dir).base.history_head.as_deref(), Some("head-b"));
    assert_eq!(
        fs::read_to_string(dir.join("source/main.dcdl")).unwrap(),
        "{ value = 3; }\n"
    );
    let diff = invoke(&retry.api(), &dir, Command::Diff).await.unwrap();
    assert!(diff.contains("update main.dcdl"));
    assert!(diff.contains("value = 3"));
}

#[tokio::test]
async fn not_observed_and_rejected_retry_do_not_release_an_uncertain_request() {
    let (_temp, dir) = checkout();
    let pending = {
        let mut local =
            LockedCheckout::open(&dir, "http://backend.test", "workspace-test").unwrap();
        let request = local.request().unwrap();
        local.state.pending = Some(request.clone());
        local.save().unwrap();
        request
    };
    let denied = serde_json::to_string(&RepositoryApiError::new(
        403,
        "forbidden",
        "secret-provider-detail",
        vec![],
    ))
    .unwrap();
    let server = Script::new(vec![
        Some(http(200, "{\"status\":\"not_observed\"}")),
        Some(http(403, &denied)),
    ])
    .await;
    assert_eq!(
        invoke(&server.api(), &dir, Command::Reconcile)
            .await
            .unwrap_err()
            .exit_code,
        7
    );
    let error = invoke(&server.api(), &dir, Command::Retry)
        .await
        .unwrap_err();
    assert_eq!(error.exit_code, 5);
    assert!(!error.to_string().contains("secret-provider-detail"));
    assert!(error.to_string().contains("unresolved"));
    assert_eq!(loaded(&dir).pending.unwrap(), pending);
}

#[tokio::test]
async fn reconciliation_after_adoption_advances_only_base_and_preserves_post_send_deletion() {
    let (_temp, dir) = checkout();
    let pending = {
        let mut local =
            LockedCheckout::open(&dir, "http://backend.test", "workspace-test").unwrap();
        let request = local.request().unwrap();
        local.state.pending = Some(request.clone());
        local.save().unwrap();
        request
    };
    fs::remove_file(dir.join("source/notes.txt")).unwrap();
    let response = ConfigurationReconciliation::Adopted {
        receipt: receipt(&pending),
    };
    let server = Script::new(vec![Some(http(
        200,
        &serde_json::to_string(&response).unwrap(),
    ))])
    .await;
    invoke(&server.api(), &dir, Command::Reconcile)
        .await
        .unwrap();
    assert!(!dir.join("source/notes.txt").exists());
    assert!(
        invoke(&server.api(), &dir, Command::Diff)
            .await
            .unwrap()
            .contains("delete notes.txt")
    );
}

#[tokio::test]
async fn dirty_pull_and_unknown_pull_never_overwrite_or_discard_a_checkout() {
    let (_temp, dir) = checkout();
    fs::write(dir.join("source/main.dcdl"), "unsaved").unwrap();
    let pending = {
        let mut local =
            LockedCheckout::open(&dir, "http://backend.test", "workspace-test").unwrap();
        let request = local.request().unwrap();
        local.state.pending = Some(request.clone());
        local.save().unwrap();
        request
    };
    let server = Script::new(vec![]).await;
    assert_eq!(
        invoke(&server.api(), &dir, Command::Pull)
            .await
            .unwrap_err()
            .exit_code,
        2
    );
    assert_eq!(
        fs::read_to_string(dir.join("source/main.dcdl")).unwrap(),
        "unsaved"
    );
    assert_eq!(loaded(&dir).pending.unwrap(), pending);
    assert!(server.received.lock().unwrap().is_empty());
}

#[tokio::test]
async fn validation_conflict_authorization_transport_and_unknown_have_distinct_exits() {
    for (status, expected) in [(400, 3), (409, 4), (401, 5), (403, 5), (500, 6)] {
        let (_temp, dir) = checkout();
        let body = serde_json::to_string(&RepositoryApiError::new(
            status,
            "error",
            "do-not-expose",
            vec![],
        ))
        .unwrap();
        let server = Script::new(vec![Some(http(status, &body))]).await;
        let error = invoke(&server.api(), &dir, Command::Validate)
            .await
            .unwrap_err();
        assert_eq!(error.exit_code, expected);
        assert!(!error.to_string().contains("do-not-expose"));
        assert!(loaded(&dir).pending.is_none());
    }
    let (_temp, dir) = checkout();
    let unknown = serde_json::to_string(&RepositoryApiError::new(
        500,
        "outcome_unknown",
        "raw-provider",
        vec![],
    ))
    .unwrap();
    let server = Script::new(vec![Some(http(200, &preview())), Some(http(500, &unknown))]).await;
    assert_eq!(
        invoke(&server.api(), &dir, Command::Push)
            .await
            .unwrap_err()
            .exit_code,
        7
    );
    assert!(loaded(&dir).pending.is_some());
}

#[tokio::test]
async fn lost_preview_response_does_not_send_or_record_a_commit() {
    let (_temp, dir) = checkout();
    let server = Script::new(vec![None]).await;
    assert_eq!(
        invoke(&server.api(), &dir, Command::Push)
            .await
            .unwrap_err()
            .exit_code,
        6
    );
    assert!(loaded(&dir).pending.is_none());
    assert_eq!(server.received.lock().unwrap().len(), 1);
}

struct RefuseCommit;
impl server_api::client_support::RequestAuthorizer for RefuseCommit {
    fn authorize(
        &self,
        request: server_api::client_support::AuthorizerRequest<'_>,
    ) -> Result<reqwest::header::HeaderMap, server_api::client_support::AuthorizationError> {
        if request.path_and_query.ends_with("/commit") {
            return Err(server_api::client_support::AuthorizationError::new());
        }
        Ok(reqwest::header::HeaderMap::new())
    }
}

#[tokio::test]
async fn interruption_before_send_keeps_durable_request_for_exact_recovery() {
    let (_temp, dir) = checkout();
    let server = Script::new(vec![Some(http(200, &preview()))]).await;
    let api = ConfigurationAdoptionApiClient::builder(&server.origin)
        .unwrap()
        .authorizer(RefuseCommit)
        .build()
        .unwrap();
    let error = run(
        &api,
        "http://backend.test",
        "workspace-test",
        &options(&dir, Command::Push),
    )
    .await
    .unwrap_err();
    assert_eq!(error.exit_code, 7);
    let pending = loaded(&dir).pending.unwrap();
    assert_eq!(server.received.lock().unwrap().len(), 1);
    let server = Script::new(vec![
        Some(http(200, "{\"status\":\"not_observed\"}")),
        Some(http(
            200,
            &serde_json::to_string(&receipt(&pending)).unwrap(),
        )),
    ])
    .await;
    assert_eq!(
        invoke(&server.api(), &dir, Command::Reconcile)
            .await
            .unwrap_err()
            .exit_code,
        7
    );
    invoke(&server.api(), &dir, Command::Retry).await.unwrap();
    let requests = server.received.lock().unwrap();
    for (_, body) in requests.iter() {
        assert_eq!(
            serde_json::from_str::<ConfigurationAdoptionRequest>(body).unwrap(),
            pending
        );
    }
}

#[tokio::test]
async fn definitive_first_commit_refusal_is_distinct_from_an_unresolved_retry() {
    for status in [400, 409, 403] {
        let (_temp, dir) = checkout();
        let refused = serde_json::to_string(&RepositoryApiError::new(
            status,
            "error",
            "not public provider text",
            vec![],
        ))
        .unwrap();
        let server = Script::new(vec![
            Some(http(200, &preview())),
            Some(http(status, &refused)),
        ])
        .await;
        let error = invoke(&server.api(), &dir, Command::Push)
            .await
            .unwrap_err();
        assert_eq!(
            error.exit_code,
            match status {
                400 => 3,
                409 => 4,
                _ => 5,
            }
        );
        assert!(loaded(&dir).pending.is_none());
        assert!(dir.join("source/main.dcdl").exists());
    }
}

#[test]
fn source_and_catalog_roundtrip_preserve_explicit_children_and_shared_set_selection() {
    // The production builtin catalog currently has no child-bearing definition.
    // This fixture checks opaque CLI preservation, not production owner/resolver authority.
    for children in [vec![], vec!["ticket.hoge.fuga"]] {
        let mut source = base();
        let catalog = serde_json::json!({"workspace":"workspace-test","namespaces":[{"namespace":"ticket","feature":"builtin:ticket","enabled":true,"declaration":{"permissions":[{"key":"ticket.hoge","description":"","children":children},{"key":"ticket.hoge.fuga","description":"","children":[]}],"sets":[{"set":"shared","children":[{"kind":"namespace","value":{"namespace":"ticket","keys":["ticket.hoge"]}}]}]}}]});
        source.catalog = serde_json::from_value(catalog.clone()).unwrap();
        source.state.snapshot.entries.get_mut("main.dcdl").unwrap().content = "{ permissions = { sets = [{ set = \"shared\"; children = [{ kind = \"namespace\"; value = { namespace = \"ticket\"; keys = [\"ticket.hoge\"]; }; }]; }]; worker_default = { sets = [\"shared\"]; }; }; } as WorkspaceConfigSchema\n".into();
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("checkout");
        install(
            &dir,
            "http://backend.test",
            "workspace-test",
            source.clone(),
        )
        .unwrap();
        let request = LockedCheckout::open(&dir, "http://backend.test", "workspace-test")
            .unwrap()
            .request()
            .unwrap();
        assert!(request.source.changes.is_empty());
        let observed = loaded(&dir);
        assert_eq!(observed.base.state, source.state);
        assert_eq!(observed.base.history_head, source.history_head);
        let metadata: serde_json::Value = read_json(&dir.join(".yoi-config/state.json")).unwrap();
        assert!(metadata["base"].get("policies").is_none());
        assert!(metadata["base"].get("migration_diagnostics").is_none());
        assert_eq!(
            serde_json::to_value(observed.base.catalog).unwrap(),
            catalog
        );
        assert_eq!(
            fs::read_to_string(dir.join("source/main.dcdl")).unwrap(),
            source.state.snapshot.entries["main.dcdl"].content
        );
    }
}
