//! Normal Server library tests for ordinary Subject MR coordination. The signed
//! Backend/Runtime, canonical policy/body/Profile/Flow, Workdirs/Git/provider,
//! independent builtin Reviewer and formal review/completion endpoints are real.
//! Only explicitly injected model decisions (including Flow verification) are
//! scripted; this is not live-model or real Host integration evidence. Runtime
//! directories must be isolated as required by the shared connected harness.
//!
//! Frozen Ticket grants derive only linked MRs and the assigned Coder's own
//! Internal children, without post-hoc IDs or wildcard grants. The full scenario
//! contrasts valid approval/published-target evidence with authority refusals.
//! Git adoption runs in a separate generic Coder's explicitly granted temp clone;
//! it never replaces the implementation assignment or touches the real Host refs.
use super::*;
use std::time::Duration;

const BODY: &str = "mr-connected-body:";
const CODER: &str = "mr-connected-coder:";
const LIMIT: Duration = Duration::from_secs(90);
const SOURCE: &str = "work/mr-connected-source";
const TARGET: &str = "mr-connected-target";
const CONTENT: &str = "ordinary Subject MR candidate verified\n";
const REVIEW_BLOCKER: &str = "Unreviewed control deliberately stops before independent review; no verdict, no adoption, no completion";

#[derive(Clone)]
struct MrModel {
    evaluated: tokio::sync::mpsc::Sender<Value>,
    seen: Arc<Mutex<std::collections::BTreeSet<String>>>,
}

// Parse raw marker JSON only, not an escaped copy inside Spawn arguments.
fn marker(request: &agen::llm_client::Request, prefix: &str) -> Option<Value> {
    request
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Message { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text),
            _ => None,
        })
        .find_map(|text| {
            text.match_indices(prefix).find_map(|(offset, _)| {
                serde_json::Deserializer::from_str(&text[offset + prefix.len()..])
                    .into_iter::<Value>()
                    .next()
                    .and_then(|value| value.ok())
            })
        })
}
fn text(request: &agen::llm_client::Request) -> String {
    request
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Message { content, .. } => Some(serde_json::to_string(content).unwrap()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn waiting(ticket: &str, row: &Value, key: &str) -> Value {
    json!({"ticket":ticket,"assignment_id":row["assignment_id"],
        "expected_last_event_id":row["last_event_id"],"operation_key":key,
        "tracking":{"status":"waiting","waiting_reason":REVIEW_BLOCKER,
            "progress":"Published source and saved blocker inspected; MR has no independent approval",
            "next_action":"Continue the independent-review path before any adoption; this control intentionally has no review",
            "result":null}})
}

impl MrModel {
    async fn coder(&self, request: &agen::llm_client::Request, input: Value) -> Vec<Event> {
        let ticket = input["ticket"].as_str().unwrap();
        let Some(requirements) = success(request, "mr-coder-requirements") else {
            return call(
                request,
                "mr-coder-requirements",
                "ShowTicket",
                json!({"id":ticket}),
            );
        };
        assert!(
            requirements["initial_post"]["body"]
                .as_str()
                .unwrap()
                .contains("MR normal requirement")
        );
        // Omission, not flow:null, selected the actual builtin recipe. Tool
        // publication alone is not evidence of this Flow being active.
        let context = format!(
            "{}\n{}",
            request.system_prompt.as_deref().unwrap_or_default(),
            text(request)
        );
        assert!(
            context.contains("This is an optional code recipe"),
            "default coder-review implement instructions missing"
        );
        assert!(
            !request
                .tools
                .iter()
                .any(|tool| tool.name == "CompleteMergeRequest")
        );
        if success(request, "mr-coder-result").is_some() {
            return done();
        }
        if let Some(mr) = success(request, "mr-coder-open") {
            // This unreviewed control intentionally ends before Reviewer spawn.
            // Save a blocker, never a fake reviewed result.
            let report = json!({"merge_request_id":mr["merge_request_id"],"reason":REVIEW_BLOCKER});
            return call(
                request,
                "mr-coder-result",
                "TicketImplementationResult",
                json!({"ticket":ticket,"kind":"blocker","body":report.to_string()}),
            );
        }
        if let Some(validation) = success(request, "mr-coder-publish") {
            assert_eq!(
                validation["output"]["terminal"]["status"], "completed",
                "{validation}"
            );
            assert_eq!(
                validation["output"]["terminal"]["exit_code"], 0,
                "{validation}"
            );
            return call(
                request,
                "mr-coder-open",
                "OpenMergeRequest",
                json!({
                "ticket":ticket,"repository_key":"test-repository", "selector_from":SOURCE,
                "selector_to":TARGET,"summary":"Real published candidate; independent review remains blocked"}),
            );
        }
        if tool_result(request, "mr-coder-write").is_some() {
            assert!(!tool_result(request, "mr-coder-write").unwrap().0);
            // One explicit source ref, no force, no tags, no target push. Git's
            // provider is the harness-owned temp repository. Verify local HEAD,
            // cleanliness and provider resolution before opening the MR.
            return call(
                request,
                "mr-coder-publish",
                "Bash",
                json!({
                "target_workdir":"checkout","cwd":".","timeout":10000,
                "command":format!("test \"$(cat README.md)\" = 'ordinary Subject MR candidate verified' && git diff --check && git add README.md && git -c user.name=Test -c user.email=test@example.invalid commit -m 'feat: mr-connected-source' && test -z \"$(git status --porcelain)\" && git push origin HEAD:refs/heads/{SOURCE} && test \"$(git rev-parse HEAD)\" = \"$(git ls-remote --exit-code --refs origin refs/heads/{SOURCE} | cut -f1)\"")}),
            );
        }
        if let Some((error, output)) = tool_result(request, "mr-coder-read") {
            assert!(!error && output.contains("clean"), "{output}");
            return call(
                request,
                "mr-coder-write",
                "Write",
                json!({
                "target_workdir":"checkout","file_path":"README.md","content":CONTENT}),
            );
        }
        call(
            request,
            "mr-coder-read",
            "Read",
            json!({"target_workdir":"checkout","file_path":"README.md"}),
        )
    }

    async fn body(&self, request: &agen::llm_client::Request, input: Value) -> Vec<Event> {
        for forbidden in ["Read", "Write", "Bash", "SubWorkerSpawn"] {
            assert!(!request.tools.iter().any(|tool| tool.name == forbidden));
        }
        for name in [
            "ShowMergeRequest",
            "CheckMergeRequestReadiness",
            "CompleteMergeRequest",
        ] {
            assert!(
                request.tools.iter().any(|tool| tool.name == name),
                "normal body must expose {name}; visibility is not a grant"
            );
        }
        let ticket = input["ticket"].as_str().unwrap();
        let Some(requirements) = success(request, "mr-body-requirements") else {
            return call(
                request,
                "mr-body-requirements",
                "ShowTicket",
                json!({"id":ticket}),
            );
        };
        assert!(requirements["content_digest"].is_string());
        assert!(
            requirements["initial_post"]["body"]
                .as_str()
                .unwrap()
                .contains("MR normal requirement")
        );
        let Some(tracking) = success(request, "mr-body-assignment") else {
            return call(
                request,
                "mr-body-assignment",
                "TicketAssignmentTrackingQuery",
                json!({"ticket":ticket}),
            );
        };
        let row = tracking.as_array().unwrap().first().unwrap();
        assert_eq!(row["principal"]["subject_id"], input["subject"]);
        let Some(spawned) = success(request, "mr-body-spawn") else {
            let mut spawn = input["spawn"].clone();
            assert!(
                spawn.get("flow").is_none(),
                "Flow must be omitted, not null"
            );
            spawn["orchestrator_assignment_id"] = row["assignment_id"].clone();
            return call(request, "mr-body-spawn", "SpawnTicketWorker", spawn);
        };
        // Discover the saved result through the real business notification, not
        // a shared model channel or test-driver WorkerNotify.
        let incoming = text(request);
        let Some(offset) = incoming.find("implementation-result:") else {
            return done();
        };
        let result_id: String = incoming[offset + "implementation-result:".len()..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
            .collect();
        assert!(!result_id.is_empty());
        let Some(saved) = success(request, "mr-body-result") else {
            return call(
                request,
                "mr-body-result",
                "TicketImplementationResultQuery",
                json!({"ticket":ticket,"result_id":result_id}),
            );
        };
        assert_eq!(saved["ticket_id"], ticket);
        assert_eq!(saved["request"]["kind"], "blocker");
        let report: Value =
            serde_json::from_str(saved["request"]["body"].as_str().unwrap()).unwrap();
        assert_eq!(report["reason"], REVIEW_BLOCKER);
        let mr_id = report["merge_request_id"].as_str().unwrap();
        let Some(mr) = success(request, "mr-body-mr") else {
            return call(
                request,
                "mr-body-mr",
                "ShowMergeRequest",
                json!({"merge_request_id":mr_id,"after":0,"limit":20}),
            );
        };
        assert_eq!(mr["merge_request_id"], mr_id);
        assert_eq!(mr["state"], "open");
        assert_eq!(mr["source"]["status"], "known", "{mr}");
        assert_eq!(mr["target"]["ref"], input["before"]);
        assert_ne!(mr["source"]["ref"], mr["target"]["ref"]);
        assert!(
            mr["thread"]
                .as_array()
                .unwrap()
                .iter()
                .all(|event| event["kind"] != "review" && event["kind"] != "merge")
        );
        let Some(readiness) = success(request, "mr-body-readiness") else {
            return call(
                request,
                "mr-body-readiness",
                "CheckMergeRequestReadiness",
                json!({"merge_request_id":mr_id}),
            );
        };
        assert_eq!(
            readiness["ready"], false,
            "unreviewed candidate cannot be adopted: {readiness}"
        );
        assert!(!readiness["blockers"].as_array().unwrap().is_empty());
        let child = json!({"kind":"runtime_worker","runtime_id":spawned["runtime_id"],"worker_id":spawned["worker_id"]});
        let Some(evidence) = success(request, "mr-body-evidence-search") else {
            return call(
                request,
                "mr-body-evidence-search",
                "SearchSessionEntries",
                json!({
                "subject":child,"query":"exit_code","tool_part":"output","limit":4}),
            );
        };
        let entries = evidence["entries"].as_array().unwrap();
        assert_eq!(
            entries.len(),
            1,
            "one concrete commit/validation/publication command"
        );
        let Some(inspected) = success(request, "mr-body-evidence-read") else {
            return call(
                request,
                "mr-body-evidence-read",
                "ReadSessionEntry",
                json!({
                "subject":child,"entry_ref":entries[0]["entry_ref"],"mode":"full"}),
            );
        };
        assert_eq!(inspected["truncated"], false);
        let evidence_text = inspected["entries"][0]["text"].as_str().unwrap();
        assert!(
            evidence_text.contains("mr-connected-source") && evidence_text.contains("exit_code"),
            "{evidence_text}"
        );
        // No approval means WAIT, not a completion call with a made-up event ID,
        // not a target Git update, not a Ticket conclusion or handling done.
        if success(request, "mr-body-wait").is_none() {
            return call(
                request,
                "mr-body-wait",
                "TicketAssignmentTrackingUpdate",
                waiting(ticket, row, "mr-body-await-independent-review"),
            );
        }
        if self.seen.lock().unwrap().insert(ticket.to_owned()) {
            self.evaluated.send(json!({"ticket":ticket,"result_id":result_id,"merge_request_id":mr_id,
                "source":mr["source"]["ref"],"requirements_digest":requirements["content_digest"],
                "decision":"do_not_adopt_unreviewed_source","evidence_ref":entries[0]["entry_ref"]})).await.unwrap();
        }
        done()
    }
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for MrModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        let events = if let Some(input) = marker(&request, CODER) {
            self.coder(&request, input).await
        } else if let Some(input) = marker(&request, BODY) {
            self.body(&request, input).await
        } else {
            done()
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}
async fn ok(h: &ConnectedWorkerHarness, method: &str, path: &str, input: Value) -> Value {
    let (status, body) = h.human(method.parse().unwrap(), path, input).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {body}");
    body
}
fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ordinary_subject_default_coder_review_publishes_mr_and_preserves_unreviewed_wait() {
    use worker_runtime::working_directory::WorkingDirectoryMaterializer;
    crate::restore_test_environment::require_restore_test_environment();
    let (evaluated, mut evaluations) = tokio::sync::mpsc::channel(2);
    let h = ConnectedWorkerHarness::start(MrModel {
        evaluated,
        seen: Default::default(),
    })
    .await;
    h.api
        .runtime
        .unregister_if_idle(EMBEDDED_WORKER_RUNTIME_ID, 100)
        .unwrap();
    // The provider, target and Coder checkout are all below h's TempDir. The
    // target is an unused branch, never the checked-out develop or real Host ref.
    let before = git(h._dir.path(), &["rev-parse", "HEAD"]);
    git(h._dir.path(), &["branch", TARGET]);
    let (status, wd) = h.human(reqwest::Method::POST,
        &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"), json!({
            "runtime_id":CONNECTED_RUNTIME,"repository_key":"test-repository", "selector":TARGET,"operation_id":"mr-connected-workdir"})).await;
    assert_eq!(status, StatusCode::CREATED, "{wd}");
    let wd = wd["item"]["working_directory_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let root = h
        .materializer
        .bind_working_directory(&wd, None)
        .unwrap()
        .root()
        .to_owned();
    git(&root, &["switch", "-c", SOURCE]);
    let (status, subject) = h
        .human(
            reqwest::Method::POST,
            &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects"),
            json!({
            "operation_id":"mr-connected-subject","role":"ordinary MR Subject","behavior_md":""}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{subject}");
    let subject = subject["id"].as_str().unwrap().to_owned();
    let subjects = open_subjektiv_store(&h.api).unwrap();
    let created_policy = subjects.current_subject_policy(&subject).unwrap().unwrap();
    assert_eq!(
        created_policy.execution,
        server_api::authorization::subject_default_permission_assignment()
    );
    assert_eq!(
        created_policy.delegation,
        server_api::authorization::subject_default_delegation_assignment()
    );
    // Enroll the unchanged creation defaults in canonical configuration. The
    // actual child grant below remains bounded to its Workdir and Ticket.
    super::super::chat_tests::adopt_chat_defaults(&h.api, &h.owner).await;
    let policy = subjects.current_subject_policy(&subject).unwrap();
    let body_path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/body");
    let body = ok(&h, "POST", &format!("{body_path}/start"), json!({
        "request_id":"mr-connected-start","expected_current":null,"expected_observation_token":null})).await;
    assert_eq!(body["disposition"], "accepted", "{body}");
    assert_eq!(
        body["body"]["observation"]["worker"]["profile"],
        "builtin:async-chat"
    );
    let backend = browser_ticket_backend(&h.api).unwrap();
    let mut ticket = NewTicket::new("Ordinary Subject MR publication and honest review blocker");
    ticket.body = "MR normal requirement: edit README, validate and publish only the source. Require an independent exact-source review before authorized isolated adoption; do not treat a blocker as a reviewed handoff.".into();
    set_test_ticket_target(&mut ticket, "test-repository", TARGET);
    let ticket = backend.create(ticket).unwrap().id;
    let assigned = ok(&h, "PUT", &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"), json!({
        "operation_id":"mr-connected-assign","expected_assignment_id":null,"principal":{"kind":"subject","subject_id":subject}})).await;
    let conversation = ok(
        &h,
        "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations"),
        json!({
        "operation_id":"mr-connected-chat","subject_id":subject}),
    )
    .await;
    let mut permissions: Vec<_> = ["filesystem.read", "filesystem.write", "bash.execute"]
        .into_iter()
        .map(|operation| ScopedPermission {
            operation: operation.into(),
            resource: PermissionResource::Workdir {
                working_directory_id: wd.clone(),
                path: ".".into(),
                recursive: true,
            },
        })
        .collect();
    permissions.extend([
        ticket_permission(&ticket, "ticket.read"),
        ticket_permission(&ticket, "ticket.relation.read"),
        ticket_permission(&ticket, "ticket.implementation_result.write"),
        ticket_permission(&ticket, "merge_request.open"),
    ]);
    let spawn = json!({"ticket_id":ticket,"runtime_id":CONNECTED_RUNTIME,"profile":"builtin:coder",
        "orchestrator_assignment_id":assigned["assignment"]["assignment_id"],
        "initial_request":format!("{CODER}{}",json!({"ticket":ticket})),
        "workdir_attachments":[{"alias":"checkout","working_directory_id":wd}],
        "execution_grant":{"permissions":permissions,"redelegation":[]}});
    assert!(spawn.get("flow").is_none());
    let sent = ok(&h, "POST", &format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations/{}/messages",conversation["id"].as_str().unwrap()), json!({
        "operation_id":"mr-connected-request", "text":format!("{BODY}{}",json!({"subject":subject,"ticket":ticket,"before":before,"spawn":spawn}))})).await;
    assert_eq!(sent["delivery"]["state"], "accepted", "{sent}");
    let evaluation = tokio::time::timeout(LIMIT, evaluations.recv())
        .await
        .expect("body must inspect the actual saved blocker, MR and committed validation evidence")
        .unwrap();
    assert_eq!(evaluation["decision"], "do_not_adopt_unreviewed_source");
    let source = git(&root, &["rev-parse", "HEAD"]);
    assert_eq!(evaluation["source"], source);
    assert_eq!(
        git(
            h._dir.path(),
            &["rev-parse", &format!("refs/heads/{SOURCE}")]
        ),
        source
    );
    assert_eq!(
        git(
            h._dir.path(),
            &["rev-parse", &format!("refs/heads/{TARGET}")]
        ),
        before
    );
    assert_eq!(git(h._dir.path(), &["rev-parse", "HEAD"]), before);
    assert_eq!(fs::read_to_string(root.join("README.md")).unwrap(), CONTENT);
    assert!(git(&root, &["status", "--porcelain"]).is_empty());
    let tracking = h
        .api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
        .unwrap()
        .remove(0);
    assert_eq!(
        tracking.assignment_id,
        assigned["assignment"]["assignment_id"].as_str().unwrap()
    );
    let tracking = tracking.tracking.unwrap();
    assert_eq!(
        tracking.status,
        ticket::TicketAssignmentTrackingStatus::Waiting
    );
    assert_eq!(tracking.waiting_reason.as_deref(), Some(REVIEW_BLOCKER));
    assert_eq!(
        subjects.current_subject_policy(&subject).unwrap(),
        policy,
        "delegation does not enlarge the parent policy"
    );
    assert_eq!(
        h.api.authority.ticket(&ticket).unwrap().state,
        server_api::BrowserTicketState::Open
    );
    let source = RuntimeWorkerRef::new(
        CONNECTED_RUNTIME,
        body["body"]["current"]["worker_id"].as_str().unwrap(),
    );
    let snapshot = h
        .factory
        .retained_session_snapshot(&worker_runtime::identity::WorkerRef::try_from(&source).unwrap())
        .unwrap();
    assert!(
        notifications(&snapshot.snapshot)
            .iter()
            .any(|message| message.contains("implementation-result:"))
    );
    let mr_id = evaluation["merge_request_id"].as_str().unwrap();
    let mr = merge_request_store(&h.api, TEST_WORKSPACE_ID)
        .unwrap()
        .get_by_id(TEST_WORKSPACE_ID, mr_id)
        .unwrap();
    assert!(mr.thread.iter().all(|event| !matches!(
        event,
        merge_request::MergeRequestThreadEvent::Review(_)
            | merge_request::MergeRequestThreadEvent::Merge(_)
    )));
    let thread_before = serde_json::to_value(&mr.thread).unwrap();
    let original_body = source;
    let published = evaluation["source"].as_str().unwrap();

    // Pre-evidence refusal ordering only: deliberately nonexistent approval and
    // unchanged target. A missing domain fence would fall through to HTTP400
    // target-evidence rejection, which is NOT accepted as authorization success.
    // The full scenario below provides the valid-evidence contrast. Do not call
    // this deliberately unreviewed control a completed review/integration case.
    let complete_path = format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests/{mr_id}/complete");
    let completion = |key: &str| {
        json!({"operation_id":key,
        "approval_event_id":"deliberately-absent-independent-verdict",
        "target_ref_before":before,"target_ref_after":published,
        "strategy":"fast_forward","resolution":"none"})
    };
    let (status, other) = h.human(reqwest::Method::POST,
        &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects"), json!({
            "operation_id":"mr-connected-other-subject","role":"other MR Subject","behavior_md":""})).await;
    assert_eq!(status, StatusCode::CREATED, "{other}");
    let other = other["id"].as_str().unwrap();
    super::super::chat_tests::adopt_subject_policy(&h.api, &h.owner, other, FULL_EXECUTION, "{}")
        .await;
    let other_body = ok(&h, "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{other}/body/start"),
        json!({"request_id":"mr-connected-other-start","expected_current":null,"expected_observation_token":null})).await;
    assert_eq!(other_body["disposition"], "accepted", "{other_body}");
    let other_worker = RuntimeWorkerRef::new(
        CONNECTED_RUNTIME,
        other_body["body"]["current"]["worker_id"].as_str().unwrap(),
    );
    let completion_permission = json!({"operation":"merge_request.complete", "resource":{
        "kind":"merge_request","workspace_id":TEST_WORKSPACE_ID,"merge_request_id":mr_id}});
    let (status, granted) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &other_worker,
        &authorization_path(),
        &completion_permission,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "positive operation-key control for the OTHER current Subject: {granted}"
    );
    let (status, error) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &other_worker,
        &complete_path,
        &completion("mr-connected-other-must-not-complete"),
    )
    .await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::CONFLICT),
        "another key-authorized current Subject must fail ownership BEFORE target/approval validation: {status} {error}"
    );

    let background = h
        .api
        .store
        .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket)
        .unwrap()
        .unwrap()
        .worker;
    let (status, error) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &background,
        &complete_path,
        &completion("mr-connected-background-must-not-complete"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "frozen child has no complete operation: {error}"
    );

    // Assign through the normal owner endpoint with its observed current row.
    // The old Subject still has complete in canonical policy; current assignment
    // ownership must be checked independently of that unchanged operation key.
    let rows = h
        .api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
        .unwrap();
    let current = serde_json::to_value(&rows[0]).unwrap();
    let changed = ok(&h, "PUT", &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),
        json!({"operation_id":"mr-connected-reassign","expected_assignment_id":current["assignment_id"],
            "principal":{"kind":"subject","subject_id":other}})).await;
    assert_ne!(
        changed["assignment"]["assignment_id"],
        current["assignment_id"]
    );
    let (status, granted) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &original_body,
        &authorization_path(),
        &completion_permission,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "reassignment does not silently remove the former Subject's operation key: {granted}"
    );
    let (status, error) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &original_body,
        &complete_path,
        &completion("mr-connected-former-subject-must-not-complete"),
    )
    .await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::CONFLICT),
        "former Subject's operation key cannot substitute for the current assignment: {status} {error}"
    );

    // Even the NEW assignee/current body must recheck canonical key exclusion.
    super::super::chat_tests::adopt_subject_policy(&h.api, &h.owner, other,
        r#"{ sets = ["conversation", "work-coordination"]; namespaces = [{ namespace = "merge_request"; keys = ["merge_request.complete"]; }]; exclude_namespaces = [{ namespace = "merge_request"; keys = ["merge_request.complete"]; }]; }"#, "{}").await;
    let (status, error) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &other_worker,
        &complete_path,
        &completion("mr-connected-revoked-must-not-complete"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "current assignee's excluded operation cannot record completion: {error}"
    );
    super::super::chat_tests::adopt_subject_policy(&h.api, &h.owner, other, "{}", "{}").await;
    let (status, error) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &other_worker,
        &complete_path,
        &completion("mr-connected-empty-must-not-complete"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "explicit empty does not restore default: {error}"
    );
    let after = merge_request_store(&h.api, TEST_WORKSPACE_ID)
        .unwrap()
        .get_by_id(TEST_WORKSPACE_ID, mr_id)
        .unwrap();
    assert_eq!(
        serde_json::to_value(&after.thread).unwrap(),
        thread_before,
        "refused completion records no MR event"
    );
    assert_eq!(
        git(
            h._dir.path(),
            &["rev-parse", &format!("refs/heads/{TARGET}")]
        ),
        before
    );
    assert_eq!(
        git(
            h._dir.path(),
            &["rev-parse", &format!("refs/heads/{SOURCE}")]
        ),
        published
    );
    assert_eq!(
        h.api.authority.ticket(&ticket).unwrap().state,
        server_api::BrowserTicketState::Open
    );
}

const FULL_BODY: &str = "full-r2-body:";
const FULL_CODER: &str = "full-r2-coder:";
const REVIEWER: &str = "full-r2-independent-reviewer:";
const ADOPTER: &str = "full-r2-adopter:";
const ADOPTED: &str = "FULL_R2_ADOPTION_PUBLISHED";
const REVIEWER_NAME: &str = "full-r2-independent-reviewer";
const FULL_SOURCE: &str = "work/full-r2-source";
const FULL_TARGET: &str = "full-r2-temp-target";
const FULL_CONTENT: &str = "full R2 independently reviewed candidate\n";
const FULL_EXECUTION: &str = r#"{ sets = ["conversation", "work-coordination"]; }"#;
const FULL_DELEGATION: &str = r#"{ sets = ["work-execution"]; }"#;

fn summary_ok(request: &agen::llm_client::Request, id: &str) -> bool {
    tool_result(request, id).is_some_and(|(error, output)| {
        assert!(!error, "{id}: {output}");
        true
    })
}
fn checked_command(request: &agen::llm_client::Request, id: &str) -> Option<Value> {
    success(request, id).inspect(|output| {
        assert_eq!(
            output["output"]["terminal"]["status"], "completed",
            "{id}: {output}"
        );
        assert_eq!(
            output["output"]["terminal"]["exit_code"], 0,
            "{id}: {output}"
        );
    })
}
fn event<'a>(mr: &'a Value, kind: &str) -> &'a Value {
    let mut events = mr["thread"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == kind);
    let found = events
        .next()
        .unwrap_or_else(|| panic!("missing {kind}: {mr}"));
    assert!(
        events.next().is_none(),
        "one {kind} for this exact-source scenario"
    );
    found
}
fn assert_approved(mr: &Value, requirements: &Value) {
    let requested = event(mr, "review_requested");
    let review = event(mr, "review");
    assert_eq!(review["decision"], "approve");
    assert_eq!(review["request_event_id"], requested["event_id"]);
    assert_eq!(review["subject_ref"], mr["source"]["ref"]);
    assert_eq!(requested["subject_ref"], review["subject_ref"]);
    assert_eq!(
        review["ticket_content_digest"],
        requirements["content_digest"]
    );
    assert_eq!(
        requested["ticket_content_digest"],
        review["ticket_content_digest"]
    );
    assert_eq!(
        requested["ticket_merge_request_subjects"],
        review["ticket_merge_request_subjects"]
    );
    let linked = review["ticket_merge_request_subjects"].as_array().unwrap();
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0]["merge_request_id"], mr["merge_request_id"]);
    assert_eq!(linked[0]["subject_ref"], review["subject_ref"]);
    assert_eq!(review["reviewer"], requested["reviewer"]);
    assert_ne!(
        review["reviewer"]["worker_id"], requested["requested_by"]["worker_id"],
        "an actual registered child, not the implementing parent, submitted the verdict"
    );
}
fn completion_input(mr: &Value, before: &Value) -> Value {
    json!({"merge_request_id":mr["merge_request_id"],"operation_id":"full-r2-exact-completion",
        "approval_event_id":event(mr,"review")["event_id"],"target_ref_before":before,
        "target_ref_after":mr["source"]["ref"],"strategy":"fast_forward","resolution":"none"})
}
fn result_id_from_notification(request: &agen::llm_client::Request) -> Option<String> {
    let incoming = text(request);
    let offset = incoming.find("implementation-result:")?;
    Some(
        incoming[offset + "implementation-result:".len()..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
            .collect(),
    )
}

#[derive(Clone)]
struct FullModel {
    stages: tokio::sync::mpsc::Sender<Value>,
    // Stage notifications schedule negative controls between ordinary body turns.
    // They never supply review/adoption evidence or issue an authority grant.
    seen: Arc<Mutex<std::collections::BTreeSet<String>>>,
}
impl FullModel {
    async fn emit_once(&self, key: &str, value: Value) {
        let fresh = self.seen.lock().unwrap().insert(key.to_owned());
        if fresh {
            self.stages.send(value).await.unwrap();
        }
    }
    fn verifier(&self, request: &agen::llm_client::Request) -> Vec<Event> {
        let input = request
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Message { content, .. } => Some(content),
                _ => None,
            })
            .flatten()
            .filter_map(|part| match part {
                ContentPart::Text { text } => serde_json::from_str::<Value>(text).ok(),
                _ => None,
            })
            .find(|value| {
                value.get("current_state").is_some() && value.get("transitions").is_some()
            })
            .expect("real Flow verifier captures its own current state/transitions");
        let state = input["current_state"].as_str().unwrap();
        let (tool, query, expected, target) = match state {
            "implement" => ("Bash", "full-r2-source-published", "exit_code", "review"),
            "review" => (
                "ShowMergeRequest",
                "approve",
                "ticket_content_digest",
                "complete",
            ),
            "complete" => (
                "TicketImplementationResult",
                "result_id",
                "result_id",
                "done",
            ),
            other => panic!("unexpected Flow state {other}: {input}"),
        };
        if summary_ok(request, "full-flow-finish") {
            return done();
        }
        let Some(found) = success(request, "full-flow-search") else {
            return call(
                request,
                "full-flow-search",
                "SearchEntries",
                json!({
                "query":query,"tool_part":"output","limit":4}),
            );
        };
        let entries = found["entries"].as_array().unwrap();
        assert!(
            !entries.is_empty(),
            "Flow condition must inspect committed evidence, not just a reason: {found}"
        );
        let Some(read) = success(request, "full-flow-read") else {
            return call(
                request,
                "full-flow-read",
                "ReadEntry",
                json!({
                "entry_ref":entries.last().unwrap()["entry_ref"],"mode":"full"}),
            );
        };
        assert_eq!(read["truncated"], false, "{read}");
        assert!(
            read.to_string().contains(expected),
            "actual captured {tool} evidence: {read}"
        );
        let results: Vec<_> = input["transitions"].as_array().unwrap().iter().map(|transition| json!({
            "transition_id":transition["transition_id"],
            "verdict":if transition["target"] == target {"met"} else {"not_met"},
            "rationale":format!("Scripted verifier inspected the committed {tool} evidence; source/approval/result binding is additionally asserted by the production caller scenario")
        })).collect();
        call(
            request,
            "full-flow-finish",
            "FinishFlowVerification",
            json!({"results":results}),
        )
    }
    async fn reviewer(&self, request: &agen::llm_client::Request, input: Value) -> Vec<Event> {
        assert!(
            !request
                .tools
                .iter()
                .any(|tool| tool.name == "OpenMergeRequest" || tool.name == "CompleteMergeRequest")
        );
        let ticket = input["ticket"].as_str().unwrap();
        let mr_id = input["merge_request_id"].as_str().unwrap();
        let Some(requirements) = success(request, "full-review-requirements") else {
            return call(
                request,
                "full-review-requirements",
                "ShowTicket",
                json!({"id":ticket}),
            );
        };
        assert!(
            requirements["initial_post"]["body"]
                .as_str()
                .unwrap()
                .contains("full R2 normal requirement")
        );
        let Some(mr) = success(request, "full-review-mr") else {
            return call(
                request,
                "full-review-mr",
                "ShowMergeRequest",
                json!({"merge_request_id":mr_id,"after":0,"limit":20}),
            );
        };
        assert_eq!(mr["source"]["status"], "known");
        let requested = event(&mr, "review_requested");
        assert_eq!(requested["subject_ref"], mr["source"]["ref"]);
        assert_eq!(
            requested["ticket_content_digest"],
            requirements["content_digest"]
        );
        if let Some(review) = success(request, "full-review-submit") {
            // ReviewMergeRequest is the public Tool. Its trusted client injects
            // the capability and calls scoped_submit_merge_request_review.
            assert_eq!(review["decision"], "approve");
            assert_eq!(review["request_event_id"], requested["event_id"]);
            assert_eq!(review["subject_ref"], mr["source"]["ref"]);
            assert_eq!(
                review["ticket_content_digest"],
                requirements["content_digest"]
            );
            return done();
        }
        if checked_command(request, "full-review-check").is_some() {
            return call(
                request,
                "full-review-submit",
                "ReviewMergeRequest",
                json!({
                "decision":"approve","body":"Scripted independent Reviewer inspected the current Ticket and trusted exact-source request, then validated committed/published HEAD and README in the actual delegated Workdir. No live-model claim.","findings":[]}),
            );
        }
        let source = mr["source"]["ref"].as_str().unwrap();
        assert!(source.bytes().all(|b| b.is_ascii_hexdigit()));
        call(
            request,
            "full-review-check",
            "Bash",
            json!({"target_workdir":"checkout","cwd":".","timeout":10000,
            "command":format!("test \"$(cat README.md)\" = 'full R2 independently reviewed candidate' && git diff --check && test -z \"$(git status --porcelain)\" && test \"$(git rev-parse HEAD)\" = '{source}' && test \"$(git ls-remote --exit-code --refs origin refs/heads/{FULL_SOURCE} | cut -f1)\" = '{source}' && git log -1 --oneline && printf 'full-r2-independent-validation\\n'")}),
        )
    }
    async fn coder(&self, request: &agen::llm_client::Request, input: Value) -> Vec<Event> {
        let ticket = input["ticket"].as_str().unwrap();
        let Some(requirements) = success(request, "full-coder-requirements") else {
            return call(
                request,
                "full-coder-requirements",
                "ShowTicket",
                json!({"id":ticket}),
            );
        };
        assert!(
            !request
                .tools
                .iter()
                .any(|tool| tool.name == "CompleteMergeRequest")
        );
        let context = format!(
            "{}\n{}",
            request.system_prompt.as_deref().unwrap_or_default(),
            text(request)
        );
        assert!(
            context.contains("optional code recipe"),
            "actual default builtin coder-review Flow missing"
        );
        if let Some(terminal) = success(request, "full-coder-flow-done") {
            assert_eq!(terminal["entered_state"], "done", "{terminal}");
            self.emit_once("coder_done",json!({"stage":"coder_done","receipt":success(request,"full-coder-result").unwrap()})).await;
            return done();
        }
        if success(request, "full-coder-result").is_some() {
            return call(
                request,
                "full-coder-flow-done",
                "RequestFlowTransition",
                json!({"reason":"The exact independently approved source and validation were saved by TicketImplementationResult; inspect its committed receipt before entering done"}),
            );
        }
        if let Some(approved) = success(request, "full-coder-flow-approved") {
            assert_eq!(approved["entered_state"], "complete", "{approved}");
            let mr = success(request, "full-coder-verdict").unwrap();
            assert_approved(&mr, &requirements);
            return call(
                request,
                "full-coder-result",
                "TicketImplementationResult",
                json!({"ticket":ticket,"kind":"result",
                "body":"full-r2 reviewed result: real edit/commit/validation/non-force source publication; actual direct builtin Reviewer formal approval; inspect retained Session evidence before isolated adoption. Scripted model choices, not live-model behavior.",
                "merge_request":{"merge_request_id":mr["merge_request_id"],"subject_ref":mr["source"]["ref"]}}),
            );
        }
        if summary_ok(request, "full-coder-reviewer-stop") {
            return call(
                request,
                "full-coder-flow-approved",
                "RequestFlowTransition",
                json!({"reason":"The actual registered direct Reviewer submitted exact-source approval; current Ticket digest and linked MR snapshot match, and the committed child session was inspected"}),
            );
        }
        if let Some(read) = success(request, "full-coder-review-read") {
            // ReadSessionEntry can bound a capture containing repeated projected
            // identities. Require the returned verdict JSON itself to be complete
            // and identical to the canonical MR event; a preview substring is not
            // sufficient evidence and the capture is not claimed untruncated.
            let rendered = read["entries"][0]["text"].as_str().unwrap();
            let content = rendered.split_once("\ncontent: ").unwrap().1;
            let retained: Value =
                serde_json::from_str(content).expect("complete retained verdict JSON");
            let mr = success(request, "full-coder-verdict").unwrap();
            let mut canonical = event(&mr, "review").clone();
            canonical.as_object_mut().unwrap().remove("kind");
            assert_eq!(retained, canonical);
            let list = success(request, "full-coder-reviewer-list").unwrap();
            let child = list["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["subject"]["kind"] == "sub_worker")
                .unwrap();
            return call(
                request,
                "full-coder-reviewer-stop",
                "WorkerStop",
                json!({"subject":child["subject"],"reason":"Exact-source formal verdict and committed child evidence inspected"}),
            );
        }
        if let Some(found) = success(request, "full-coder-review-search") {
            let entries = found["entries"].as_array().unwrap();
            assert!(
                !entries.is_empty(),
                "committed formal verdict evidence: {found}"
            );
            let list = success(request, "full-coder-reviewer-list").unwrap();
            let child = list["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["subject"]["kind"] == "sub_worker")
                .unwrap();
            let formal = entries.last().unwrap();
            return call(
                request,
                "full-coder-review-read",
                "ReadSessionEntry",
                json!({"subject":child["subject"],"entry_ref":formal["entry_ref"],"mode":"full"}),
            );
        }
        if let Some(list) = success(request, "full-coder-reviewer-list") {
            let children: Vec<_> = list["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["subject"]["kind"] == "sub_worker")
                .collect();
            assert_eq!(children.len(), 1, "one independent direct child");
            assert_eq!(children[0]["relation"], "direct_child");
            assert_eq!(children[0]["subject"]["name"], REVIEWER_NAME);
            return call(
                request,
                "full-coder-review-search",
                "SearchSessionEntries",
                json!({
                "subject":children[0]["subject"],"query":"Scripted independent Reviewer inspected","tool_part":"output","limit":4}),
            );
        }
        if let Some(mr) = success(request, "full-coder-verdict") {
            assert_approved(&mr, &requirements);
            return call(request, "full-coder-reviewer-list", "WorkerList", json!({}));
        }
        if summary_ok(request, "full-coder-spawn-reviewer") {
            // Only the real child's ordinary turn-ended notification wakes this
            // branch. No shared test channel supplies a verdict to the Coder.
            let incoming = text(request);
            if !incoming.contains(&format!("SubWorker `{REVIEWER_NAME}` turn ended")) {
                return done();
            }
            let mr = success(request, "full-coder-pre-review-mr").unwrap();
            return call(
                request,
                "full-coder-verdict",
                "ShowMergeRequest",
                json!({"merge_request_id":mr["merge_request_id"],"after":0,"limit":20}),
            );
        }
        if let Some(entered) = success(request, "full-coder-flow-review") {
            assert_eq!(entered["entered_state"], "review", "{entered}");
            let mr = success(request, "full-coder-pre-review-mr").unwrap();
            return call(
                request,
                "full-coder-spawn-reviewer",
                "SubWorkerSpawn",
                json!({
                "name":REVIEWER_NAME,"profile":"builtin:reviewer","command":true,
                "scope":[{"target_workdir":"checkout","target":".","permission":"write"}],
                "review":{"ticket_id":ticket,"merge_request_id":mr["merge_request_id"]},
                // No source identity, capability token or prewritten verdict.
                "task":format!("{REVIEWER}{}",json!({"ticket":ticket,"merge_request_id":mr["merge_request_id"]}))}),
            );
        }
        if success(request, "full-coder-pre-review-mr").is_some() {
            return call(
                request,
                "full-coder-flow-review",
                "RequestFlowTransition",
                json!({"reason":"Implementation is committed, clean, narrowly validated and published source equals HEAD; inspect the committed Bash/provider evidence before review"}),
            );
        }
        if let Some(mr) = success(request, "full-coder-open") {
            return call(
                request,
                "full-coder-pre-review-mr",
                "ShowMergeRequest",
                json!({"merge_request_id":mr["merge_request_id"],"after":0,"limit":20}),
            );
        }
        if checked_command(request, "full-coder-publish").is_some() {
            return call(
                request,
                "full-coder-open",
                "OpenMergeRequest",
                json!({"ticket":ticket,"repository_key":"test-repository",
                "selector_from":FULL_SOURCE,"selector_to":FULL_TARGET,"summary":"full R2 production-connected independently reviewed isolated candidate"}),
            );
        }
        if summary_ok(request, "full-coder-write") {
            return call(
                request,
                "full-coder-publish",
                "Bash",
                json!({"target_workdir":"checkout","cwd":".","timeout":10000,
                "command":format!("test \"$(cat README.md)\" = 'full R2 independently reviewed candidate' && git diff --check && git add README.md && git -c user.name=Test -c user.email=test@example.invalid commit -m 'feat: full-r2-source' && test -z \"$(git status --porcelain)\" && git push origin HEAD:refs/heads/{FULL_SOURCE} && test \"$(git rev-parse HEAD)\" = \"$(git ls-remote --exit-code --refs origin refs/heads/{FULL_SOURCE} | cut -f1)\" && printf 'full-r2-source-published\\n'")}),
            );
        }
        if let Some((error, output)) = tool_result(request, "full-coder-read") {
            assert!(!error && output.contains("clean"), "{output}");
            return call(
                request,
                "full-coder-write",
                "Write",
                json!({"target_workdir":"checkout","file_path":"README.md","content":FULL_CONTENT}),
            );
        }
        call(
            request,
            "full-coder-read",
            "Read",
            json!({"target_workdir":"checkout","file_path":"README.md"}),
        )
    }
    async fn adopter(&self, request: &agen::llm_client::Request, input: Value) -> Vec<Event> {
        if summary_ok(request, "full-adopter-notify") {
            self.emit_once("adopter_done",json!({"stage":"adopter_done","receipt":success(request,"full-adopter-notify").unwrap()})).await;
            return done();
        }
        if checked_command(request, "full-adopter-git").is_some() {
            return call(
                request,
                "full-adopter-notify",
                "TicketImplementationResult",
                json!({"ticket":input["ticket"],
                "kind":"result","body":format!("{ADOPTED}:{}",input["ticket"].as_str().unwrap())}),
            );
        }
        let Some(requirements) = success(request, "full-adopter-requirements") else {
            return call(
                request,
                "full-adopter-requirements",
                "ShowTicket",
                json!({"id":input["ticket"]}),
            );
        };
        assert!(
            requirements["initial_post"]["body"]
                .as_str()
                .unwrap()
                .contains("full R2 normal requirement")
        );
        let source = input["source"].as_str().unwrap();
        let before = input["before"].as_str().unwrap();
        for reference in [source, before] {
            assert_eq!(reference.len(), 40);
            assert!(reference.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        // This separate temp clone has no implementation assignment and never
        // touches the source branch. The origin is a harness-only local path.
        call(
            request,
            "full-adopter-git",
            "Bash",
            json!({"target_workdir":"workdir","cwd":".","timeout":10000,
            "command":format!("test \"$(git remote get-url origin)\" = {} && test -z \"$(git status --porcelain)\" && test \"$(git rev-parse HEAD)\" = '{before}' && test \"$(git ls-remote --exit-code --refs origin refs/heads/{FULL_TARGET} | cut -f1)\" = '{before}' && git fetch --no-tags origin refs/heads/{FULL_SOURCE} && test \"$(git rev-parse FETCH_HEAD)\" = '{source}' && git switch -c {FULL_TARGET} && git merge --ff-only '{source}' && test \"$(cat README.md)\" = 'full R2 independently reviewed candidate' && git diff --check && test -z \"$(git status --porcelain)\" && git push origin HEAD:refs/heads/{FULL_TARGET} && test \"$(git ls-remote --exit-code --refs origin refs/heads/{FULL_TARGET} | cut -f1)\" = '{source}' && printf 'full-r2-adopt-published\\n'", shell_quote(input["origin"].as_str().unwrap()))}),
        )
    }
    async fn body(&self, request: &agen::llm_client::Request, input: Value) -> Vec<Event> {
        for forbidden in ["Read", "Write", "Bash", "SubWorkerSpawn"] {
            assert!(!request.tools.iter().any(|tool| tool.name == forbidden));
        }
        let ticket = input["ticket"].as_str().unwrap();
        let Some(requirements) = success(request, "full-body-requirements") else {
            return call(
                request,
                "full-body-requirements",
                "ShowTicket",
                json!({"id":ticket}),
            );
        };
        let Some(tracking) = success(request, "full-body-assignment") else {
            return call(
                request,
                "full-body-assignment",
                "TicketAssignmentTrackingQuery",
                json!({"ticket":ticket}),
            );
        };
        let row = tracking.as_array().unwrap().first().unwrap();
        assert_eq!(row["principal"]["subject_id"], input["subject"]);
        let Some(coder) = success(request, "full-body-spawn-coder") else {
            let mut spawn = input["spawn"].clone();
            assert!(spawn.get("flow").is_none());
            spawn["orchestrator_assignment_id"] = row["assignment_id"].clone();
            return call(request, "full-body-spawn-coder", "SpawnTicketWorker", spawn);
        };
        let Some(result_id) = result_id_from_notification(request) else {
            return done();
        };
        let Some(saved) = success(request, "full-body-result") else {
            return call(
                request,
                "full-body-result",
                "TicketImplementationResultQuery",
                json!({"ticket":ticket,"result_id":result_id}),
            );
        };
        assert_eq!(saved["ticket_id"], ticket);
        assert_eq!(saved["request"]["kind"], "result");
        let mr_id = saved["request"]["merge_request"]["merge_request_id"]
            .as_str()
            .unwrap();
        let Some(mr) = success(request, "full-body-reviewed-mr") else {
            return call(
                request,
                "full-body-reviewed-mr",
                "ShowMergeRequest",
                json!({"merge_request_id":mr_id,"after":0,"limit":20}),
            );
        };
        assert_approved(&mr, &requirements);
        assert_eq!(
            saved["request"]["merge_request"]["subject_ref"],
            mr["source"]["ref"]
        );
        assert_eq!(mr["target"]["ref"], input["before"]);
        let Some(readiness) = success(request, "full-body-readiness") else {
            return call(
                request,
                "full-body-readiness",
                "CheckMergeRequestReadiness",
                json!({"merge_request_id":mr_id}),
            );
        };
        assert_eq!(readiness["ready"], true, "{readiness}");
        let coder_subject = json!({"kind":"runtime_worker","runtime_id":coder["runtime_id"],"worker_id":coder["worker_id"]});
        let Some(found) = success(request, "full-body-code-search") else {
            return call(
                request,
                "full-body-code-search",
                "SearchSessionEntries",
                json!({"subject":coder_subject,
                "query":"full-r2-source-published","tool_part":"output","limit":4}),
            );
        };
        let Some(read) = success(request, "full-body-code-read") else {
            let entries = found["entries"].as_array().unwrap();
            assert_eq!(entries.len(), 1);
            return call(
                request,
                "full-body-code-read",
                "ReadSessionEntry",
                json!({"subject":coder_subject,"entry_ref":entries[0]["entry_ref"],"mode":"full"}),
            );
        };
        assert_eq!(read["truncated"], false);
        assert!(read.to_string().contains("exit_code"));
        let Some(adopter) = success(request, "full-body-spawn-adopter") else {
            // The Subject has independently inspected the saved result, exact
            // structured verdict, readiness and command evidence before choosing
            // full-source adoption. No test-driver boolean grants permission.
            let mut spawn = input["adoption_spawn"].clone();
            let command_input = json!({"ticket":ticket,"source":mr["source"]["ref"],"before":input["before"],
                "origin":input["origin"],"body_worker":input["body_worker"]});
            spawn["initial_submit"] = serde_json::to_value(vec![protocol::Segment::text(format!(
                "{ADOPTER}{command_input}"
            ))])
            .unwrap();
            return call(request, "full-body-spawn-adopter", "WorkerSpawn", spawn);
        };
        let incoming = text(request);
        let Some(offset) = incoming.rfind("implementation-result:") else {
            return done();
        };
        let adoption_result: String = incoming[offset + "implementation-result:".len()..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
            .collect();
        if adoption_result == result_id {
            return done();
        }
        let Some(adoption_saved) = success(request, "full-body-adoption-result") else {
            return call(
                request,
                "full-body-adoption-result",
                "TicketImplementationResultQuery",
                json!({"ticket":ticket,"result_id":adoption_result}),
            );
        };
        assert_eq!(adoption_saved["ticket_id"], ticket);
        assert_eq!(adoption_saved["request"]["kind"], "result");
        assert_eq!(
            adoption_saved["request"]["body"],
            format!("{ADOPTED}:{ticket}")
        );
        let adopter_subject = json!({"kind":"runtime_worker","runtime_id":adopter["runtime_id"],"worker_id":adopter["worker_id"]});
        let Some(adoption) = success(request, "full-body-adopt-search") else {
            return call(
                request,
                "full-body-adopt-search",
                "SearchSessionEntries",
                json!({"subject":adopter_subject,
                "query":"full-r2-adopt-published","tool_part":"output","limit":4}),
            );
        };
        let Some(adoption_read) = success(request, "full-body-adopt-read") else {
            let entries = adoption["entries"].as_array().unwrap();
            assert_eq!(entries.len(), 1);
            return call(
                request,
                "full-body-adopt-read",
                "ReadSessionEntry",
                json!({"subject":adopter_subject,"entry_ref":entries[0]["entry_ref"],"mode":"full"}),
            );
        };
        assert_eq!(adoption_read["truncated"], false);
        assert!(adoption_read.to_string().contains("exit_code"));
        let Some(published) = success(request, "full-body-published-target") else {
            return call(
                request,
                "full-body-published-target",
                "ShowMergeRequest",
                json!({"merge_request_id":mr_id,"after":0,"limit":20}),
            );
        };
        assert_approved(&published, &requirements);
        assert_eq!(published["source"]["ref"], mr["source"]["ref"]);
        assert_eq!(published["target"]["status"], "known");
        assert_eq!(published["target"]["ref"], mr["source"]["ref"]);
        let completion = completion_input(&published, &input["before"]);
        let fresh = self
            .seen
            .lock()
            .unwrap()
            .insert("completion_rendezvous".into());
        if fresh {
            self.stages.send(json!({"stage":"ready_to_complete","completion":completion,"coder":coder_subject,
                "adopter":adopter_subject,"result_id":result_id,"source":published["source"]["ref"],
                "approval":event(&published,"review"),"requirements_digest":requirements["content_digest"]})).await.unwrap();
        }
        // End this ordinary turn before policy-revocation controls. Keeping a
        // model-supply future parked across revocation would race legitimate
        // cancellation. The later ordinary conversation message merely schedules
        // continuation; it is not evidence, an approval or an authority gate.
        if !text(request).contains(&format!("FULL_R2_COMPLETION_CONTINUE:{ticket}")) {
            return done();
        }
        let Some(current_requirements) = success(request, "full-body-final-requirements") else {
            return call(
                request,
                "full-body-final-requirements",
                "ShowTicket",
                json!({"id":ticket}),
            );
        };
        assert_eq!(
            current_requirements["content_digest"],
            requirements["content_digest"]
        );
        let Some(final_tracking) = success(request, "full-body-final-assignment") else {
            return call(
                request,
                "full-body-final-assignment",
                "TicketAssignmentTrackingQuery",
                json!({"ticket":ticket}),
            );
        };
        let current = final_tracking.as_array().unwrap().first().unwrap();
        assert_eq!(current["principal"]["subject_id"], input["subject"]);
        let Some(current_mr) = success(request, "full-body-current-mr") else {
            return call(
                request,
                "full-body-current-mr",
                "ShowMergeRequest",
                json!({"merge_request_id":mr_id,"after":0,"limit":20}),
            );
        };
        assert_approved(&current_mr, &current_requirements);
        assert_eq!(current_mr["target"]["ref"], current_mr["source"]["ref"]);
        let Some(current_readiness) = success(request, "full-body-current-readiness") else {
            return call(
                request,
                "full-body-current-readiness",
                "CheckMergeRequestReadiness",
                json!({"merge_request_id":mr_id}),
            );
        };
        assert_eq!(current_readiness["ready"], true, "{current_readiness}");
        let completion = completion_input(&current_mr, &input["before"]);
        let Some(completed) = success(request, "full-body-complete") else {
            return call(
                request,
                "full-body-complete",
                "CompleteMergeRequest",
                completion.clone(),
            );
        };
        let Some(replay) = success(request, "full-body-complete-replay") else {
            return call(
                request,
                "full-body-complete-replay",
                "CompleteMergeRequest",
                completion,
            );
        };
        assert_eq!(
            replay, completed,
            "exact replay retains the original merge event"
        );
        let Some(final_mr) = success(request, "full-body-final-mr") else {
            return call(
                request,
                "full-body-final-mr",
                "ShowMergeRequest",
                json!({"merge_request_id":mr_id,"after":0,"limit":20}),
            );
        };
        assert_eq!(final_mr["state"], "merged");
        assert_eq!(event(&final_mr, "merge")["event_id"], completed["event_id"]);
        if success(request, "full-body-done").is_none() {
            return call(
                request,
                "full-body-done",
                "TicketAssignmentTrackingUpdate",
                json!({"ticket":ticket,
                "assignment_id":current["assignment_id"],"expected_last_event_id":current["last_event_id"],
                "operation_key":"full-r2-body-done","tracking":{"status":"done","waiting_reason":null,
                "progress":null,"next_action":null,"result":"Independently reviewed exact source adopted only in isolated temp provider; typed completion and exact replay inspected. Ticket case/real Host were not changed."}}),
            );
        }
        self.emit_once(
            "body_done",
            json!({"stage":"body_done","completion":completed,"result_id":result_id}),
        )
        .await;
        done()
    }
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for FullModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        let events = if request
            .tools
            .iter()
            .any(|tool| tool.name == "FinishFlowVerification")
        {
            self.verifier(&request)
        } else if let Some(input) = marker(&request, REVIEWER) {
            self.reviewer(&request, input).await
        } else if let Some(input) = marker(&request, ADOPTER) {
            self.adopter(&request, input).await
        } else if let Some(input) = marker(&request, FULL_CODER) {
            self.coder(&request, input).await
        } else if let Some(input) = marker(&request, FULL_BODY) {
            self.body(&request, input).await
        } else {
            done()
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

async fn body_idle_now(h: &ConnectedWorkerHarness, worker: &RuntimeWorkerRef) {
    let mut observed = h.runtime.subscribe_worker_observation().unwrap();
    if h.api.runtime.worker(worker).unwrap().state == "idle" {
        return;
    }
    tokio::time::timeout(LIMIT, async {
        loop {
            let event = observed.recv().await.expect("real body observation");
            if event.worker_ref.worker_id.to_string() != worker.worker_id {
                continue;
            }
            match event.payload {
                protocol::Event::WorkerState { snapshot }
                    if snapshot.state == protocol::WorkerState::Idle =>
                {
                    break;
                }
                protocol::Event::Error { code, message } => {
                    panic!("body failed before controls: {code:?} {message}")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("body turn must actually finish before policy controls");
}

async fn normal_subject(
    h: &ConnectedWorkerHarness,
    key: &str,
    execution: &str,
    delegation: &str,
) -> (String, Value) {
    let (status, created) = h
        .human(
            reqwest::Method::POST,
            &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects"),
            json!({
            "operation_id":format!("{key}-create"),"role":key,"behavior_md":""}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let subject = created["id"].as_str().unwrap().to_owned();
    let policy = open_subjektiv_store(&h.api)
        .unwrap()
        .current_subject_policy(&subject)
        .unwrap()
        .unwrap();
    assert_eq!(
        policy.execution,
        server_api::authorization::subject_default_permission_assignment()
    );
    assert_eq!(
        policy.delegation,
        server_api::authorization::subject_default_delegation_assignment()
    );
    if execution == FULL_EXECUTION && delegation == FULL_DELEGATION {
        // The positive scenario keeps both actual creation assignments intact.
        super::super::chat_tests::adopt_chat_defaults(&h.api, &h.owner).await;
        assert_eq!(
            open_subjektiv_store(&h.api)
                .unwrap()
                .current_subject_policy(&subject)
                .unwrap()
                .unwrap(),
            policy
        );
    } else {
        super::super::chat_tests::adopt_subject_policy(
            &h.api, &h.owner, &subject, execution, delegation,
        )
        .await;
    }
    let started = ok(h,"POST",&format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/body/start"),json!({
        "request_id":format!("{key}-start"),"expected_current":null,"expected_observation_token":null})).await;
    assert_eq!(started["disposition"], "accepted", "{started}");
    assert_eq!(
        started["body"]["observation"]["worker"]["profile"],
        "builtin:async-chat"
    );
    (subject, started["body"].clone())
}
async fn temp_workdir(h: &ConnectedWorkerHarness, key: &str) -> (String, PathBuf) {
    use worker_runtime::working_directory::WorkingDirectoryMaterializer;
    let (status,created) = h.human(reqwest::Method::POST,
        &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),json!({
            "runtime_id":CONNECTED_RUNTIME,"repository_key":"test-repository","selector":FULL_TARGET,"operation_id":key})).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["item"]["working_directory_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let root = h
        .materializer
        .bind_working_directory(&id, None)
        .unwrap()
        .root()
        .to_owned();
    (id, root)
}
fn workdir_permissions(id: &str) -> Vec<ScopedPermission> {
    ["filesystem.read", "filesystem.write", "bash.execute"]
        .into_iter()
        .map(|operation| ScopedPermission {
            operation: operation.into(),
            resource: PermissionResource::Workdir {
                working_directory_id: id.into(),
                path: ".".into(),
                recursive: true,
            },
        })
        .collect()
}
fn permission_for_complete(mr: &str) -> Value {
    json!({"operation":"merge_request.complete","resource":{
        "kind":"merge_request","workspace_id":TEST_WORKSPACE_ID,"merge_request_id":mr}})
}
async fn completion_denied(
    h: &ConnectedWorkerHarness,
    worker: &RuntimeWorkerRef,
    wire: &Value,
    key: &str,
    expect_key: bool,
) {
    let mr = wire["merge_request_id"].as_str().unwrap();
    if expect_key {
        let (status, value) = connected_authorization_post(
            &h.backend_url,
            &h.identity,
            worker,
            &authorization_path(),
            &permission_for_complete(mr),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{key}: operation is granted independently of Subject ownership: {value}"
        );
    }
    let mut body = wire.clone();
    body.as_object_mut().unwrap().remove("merge_request_id");
    body["operation_id"] = json!(key);
    let (status, error) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        worker,
        &format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests/{mr}/complete"),
        &body,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{key}: valid exact-source approval and published target must not substitute for current Subject/body/policy: {error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ordinary_subject_full_mr_direct_reviewer_saved_result_adoption_completion_and_authority_refusals()
 {
    full_mr_scenario(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ordinary_subject_mr_valid_approval_rejects_retained_old_body_after_explicit_replacement() {
    full_mr_scenario(true).await;
}

async fn replace_idle_body(h: &ConnectedWorkerHarness, subject: &str) -> RuntimeWorkerRef {
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/body");
    let observed = ok(h, "GET", &path, Value::Null).await;
    let old = RuntimeWorkerRef::new(
        CONNECTED_RUNTIME,
        observed["current"]["worker_id"].as_str().unwrap(),
    );
    body_idle_now(h, &old).await;
    // The release owns an actual live Controller reservation and producer source
    // inventory. Never stop first or synthesize a lease/Host drain receipt.
    let release_api = h.api.clone();
    let release_worker = old.clone();
    let released = tokio::time::timeout(
        LIMIT,
        tokio::task::spawn_blocking(move || {
            subjektiv_journal::release_subject(
                &release_api,
                serde_json::from_value(json!({
            "worker":release_worker,"operation_id":"full-r2-explicit-idle-body-release"}))
                .unwrap(),
            )
        }),
    )
    .await
    .expect("bounded production Runtime release proof")
    .unwrap()
    .unwrap();
    assert_eq!(
        released.state,
        runtime_api::SubjectReleaseState::Disconnected,
        "{released:?}"
    );
    let absent = ok(h, "GET", &path, Value::Null).await;
    assert!(
        absent["current"].is_null(),
        "proof-backed release ends old lease: {absent}"
    );
    let replacement = ok(
        h,
        "POST",
        &format!("{path}/start"),
        json!({"request_id":"full-r2-replacement-start",
        "expected_current":null,"expected_observation_token":null}),
    )
    .await;
    assert_eq!(replacement["disposition"], "accepted", "{replacement}");
    assert_ne!(replacement["body"]["current"]["worker_id"], old.worker_id);
    assert_eq!(
        replacement["body"]["observation"]["worker"]["profile"],
        "builtin:async-chat"
    );
    RuntimeWorkerRef::new(
        CONNECTED_RUNTIME,
        replacement["body"]["current"]["worker_id"]
            .as_str()
            .unwrap(),
    )
}

async fn full_mr_scenario(replace_body: bool) {
    crate::restore_test_environment::require_restore_test_environment();
    let (stages, mut events) = tokio::sync::mpsc::channel(12);
    let h = ConnectedWorkerHarness::start(FullModel {
        stages,
        seen: Default::default(),
    })
    .await;
    // The product Runtime bootstrap binds its transport identity before source
    // publication/release. The shared older connection fixture omits that step.
    h.runtime.bind_runtime_identity(CONNECTED_RUNTIME).unwrap();
    h.api
        .runtime
        .unregister_if_idle(EMBEDDED_WORKER_RUNTIME_ID, 100)
        .unwrap();
    let before = git(h._dir.path(), &["rev-parse", "HEAD"]);
    git(h._dir.path(), &["branch", FULL_TARGET]);
    let (coder_wd, coder_root) = temp_workdir(&h, "full-r2-coder-workdir").await;
    let (adopt_wd, adopt_root) = temp_workdir(&h, "full-r2-adoption-workdir").await;
    git(&coder_root, &["switch", "-c", FULL_SOURCE]);
    // No adoption command runs in either the Server provider checkout or the
    // implementation checkout. The generic adopter owns this second temp clone.
    assert_ne!(coder_root, adopt_root);
    let (subject, body) =
        normal_subject(&h, "full-r2-owner", FULL_EXECUTION, FULL_DELEGATION).await;
    let (other,other_body) = normal_subject(&h,"full-r2-other",
        r#"{ sets = ["conversation", "work-coordination"]; namespaces = [{ namespace = "merge_request"; keys = ["merge_request.complete"]; }]; }"#,"{}").await;
    let owner_worker = RuntimeWorkerRef::new(
        CONNECTED_RUNTIME,
        body["current"]["worker_id"].as_str().unwrap(),
    );
    let other_worker = RuntimeWorkerRef::new(
        CONNECTED_RUNTIME,
        other_body["current"]["worker_id"].as_str().unwrap(),
    );
    let replacement = if replace_body {
        Some(replace_idle_body(&h, &other).await)
    } else {
        None
    };
    let backend = browser_ticket_backend(&h.api).unwrap();
    let mut ticket = NewTicket::new("Full R2 ordinary Subject isolated reviewed MR adoption");
    ticket.body = "full R2 normal requirement: change README to 'full R2 independently reviewed candidate', commit, validate, publish only the source and use the actual direct builtin Reviewer for a formal exact-source verdict. The current assigned Subject may fully adopt the approved source only in the explicitly authorized second temporary clone/provider target, then record completion. Do not touch the real Host target; keep Ticket conclusion separate.".into();
    set_test_ticket_target(&mut ticket, "test-repository", FULL_TARGET);
    let ticket = backend.create(ticket).unwrap().id;
    let assignment = ok(&h,"PUT",&format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),json!({
        "operation_id":"full-r2-assign","expected_assignment_id":null,"principal":{"kind":"subject","subject_id":subject}})).await;
    let conversation = ok(
        &h,
        "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations"),
        json!({
        "operation_id":"full-r2-chat","subject_id":subject}),
    )
    .await;
    let mut coder_permissions = workdir_permissions(&coder_wd);
    // Frozen Ticket contexts derive only this Ticket's
    // linked MRs from these frozen Ticket
    // contexts. There is no future MR ID, allocated Worker ID or wildcard here.
    for operation in [
        "ticket.read",
        "ticket.relation.read",
        "ticket.implementation_result.write",
        "merge_request.open",
        "merge_request.read",
        "merge_request.readiness",
        "merge_request.review",
    ] {
        coder_permissions.push(ticket_permission(&ticket, operation));
    }
    coder_permissions.push(collection("worker.list"));
    let reviewer_redelegation: Vec<_> = coder_permissions
        .iter()
        .filter(|permission| {
            matches!(
                permission.operation.as_str(),
                "filesystem.read"
                    | "filesystem.write"
                    | "bash.execute"
                    | "ticket.read"
                    | "ticket.relation.read"
                    | "merge_request.read"
                    | "merge_request.review"
            )
        })
        .cloned()
        .collect();
    let spawn = json!({"ticket_id":ticket,"runtime_id":CONNECTED_RUNTIME,"profile":"builtin:coder",
        "orchestrator_assignment_id":assignment["assignment"]["assignment_id"],
        "initial_request":format!("{FULL_CODER}{}",json!({"ticket":ticket})),
        "workdir_attachments":[{"alias":"checkout","working_directory_id":coder_wd}],
        "execution_grant":{"permissions":coder_permissions,"redelegation":reviewer_redelegation}});
    assert!(
        spawn.get("flow").is_none(),
        "default builtin coder-review is selected by omission"
    );
    let mut adopter_permissions = workdir_permissions(&adopt_wd);
    adopter_permissions.extend([
        ticket_permission(&ticket, "ticket.read"),
        ticket_permission(&ticket, "ticket.relation.read"),
        ticket_permission(&ticket, "ticket.implementation_result.write"),
    ]);
    let adoption_spawn = json!({"runtime_id":CONNECTED_RUNTIME,"working_directory_id":adopt_wd,
        "operation_id":"full-r2-generic-adopter","profile":"builtin:coder","ticket_id":null,
        "display_name":"Bounded isolated exact-source Git adopter","initial_submit":[],
        "execution_grant":{"permissions":adopter_permissions,"redelegation":[]}});
    let input = json!({"subject":subject,"ticket":ticket,"spawn":spawn,"adoption_spawn":adoption_spawn,
        "before":before,"origin":h._dir.path().display().to_string(),"body_worker":target(&owner_worker)});
    let sent = ok(
        &h,
        "POST",
        &format!(
            "/api/w/{TEST_WORKSPACE_ID}/chat/conversations/{}/messages",
            conversation["id"].as_str().unwrap()
        ),
        json!({
        "operation_id":"full-r2-request","text":format!("{FULL_BODY}{input}")}),
    )
    .await;
    assert_eq!(sent["delivery"]["state"], "accepted", "{sent}");
    let mut observed = std::collections::BTreeMap::<String, Value>::new();
    tokio::time::timeout(LIMIT,async {
        while !["coder_done","adopter_done","ready_to_complete"].iter().all(|stage| observed.contains_key(*stage)) {
            let event = events.recv().await.expect("real model stages");
            observed.insert(event["stage"].as_str().unwrap().to_owned(),event);
        }
    }).await.expect("actual review/Flow/result/decision/adoption must all reach the bounded completion rendezvous");
    body_idle_now(&h, &owner_worker).await;
    let ready = &observed["ready_to_complete"];
    let completion = &ready["completion"];
    let mr_id = completion["merge_request_id"].as_str().unwrap();
    let source = ready["source"].as_str().unwrap();
    assert_eq!(
        observed["coder_done"]["receipt"]["result_id"],
        ready["result_id"]
    );
    assert_eq!(git(&coder_root, &["rev-parse", "HEAD"]), source);
    assert_eq!(git(&adopt_root, &["rev-parse", "HEAD"]), source);
    assert_eq!(
        git(
            h._dir.path(),
            &["rev-parse", &format!("refs/heads/{FULL_SOURCE}")]
        ),
        source
    );
    assert_eq!(
        git(
            h._dir.path(),
            &["rev-parse", &format!("refs/heads/{FULL_TARGET}")]
        ),
        source
    );
    assert_eq!(
        git(h._dir.path(), &["rev-parse", "HEAD"]),
        before,
        "provider's checked-out branch is untouched"
    );
    assert_eq!(
        fs::read_to_string(h._dir.path().join("README.md")).unwrap(),
        "clean\n"
    );
    let mr_store = merge_request_store(&h.api, TEST_WORKSPACE_ID).unwrap();
    let reviewed = mr_store.get_by_id(TEST_WORKSPACE_ID, mr_id).unwrap();
    let before_events = serde_json::to_value(&reviewed.thread).unwrap();
    assert!(
        reviewed
            .thread
            .iter()
            .all(|event| !matches!(event, merge_request::MergeRequestThreadEvent::Merge(_)))
    );
    let implementation = h
        .api
        .store
        .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket)
        .unwrap()
        .unwrap();
    let coder = RuntimeWorkerRef::new(
        ready["coder"]["runtime_id"].as_str().unwrap(),
        ready["coder"]["worker_id"].as_str().unwrap(),
    );
    assert_eq!(
        implementation.worker, coder,
        "generic Git adopter must NOT replace the implementation assignment"
    );

    // All completion evidence is now valid: actual independent source/digest
    // approval + externally applied exact target. These are domain refusals,
    // not failures explained by missing approval or target publication.
    completion_denied(
        &h,
        replacement.as_ref().unwrap_or(&other_worker),
        completion,
        "full-r2-other-current-subject-denied",
        true,
    )
    .await;
    // Frozen implementation grants deliberately do NOT include completion.
    // With otherwise valid review/target evidence the background is still denied;
    // the operation-positive current-body ownership contrast is the other/former
    // Subject above/below, plus the independent admission component tests.
    completion_denied(
        &h,
        &coder,
        completion,
        "full-r2-frozen-background-denied",
        false,
    )
    .await;
    let rows = h
        .api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
        .unwrap();
    let old_row = serde_json::to_value(&rows[0]).unwrap();
    let moved = ok(&h,"PUT",&format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),json!({
        "operation_id":"full-r2-temporary-reassignment","expected_assignment_id":old_row["assignment_id"],
        "principal":{"kind":"subject","subject_id":other}})).await;
    completion_denied(
        &h,
        &owner_worker,
        completion,
        "full-r2-former-assignee-with-current-key-denied",
        true,
    )
    .await;
    if let Some(replacement) = &replacement {
        // The Ticket is currently assigned to the replaced body's Subject.
        assert!(
            h.api
                .store
                .get_worker_registry(TEST_WORKSPACE_ID, &other_worker)
                .unwrap()
                .is_some(),
            "old source remains retained; refusal cannot be explained by deletion"
        );
        let (status, control) = connected_authorization_post(
            &h.backend_url,
            &h.identity,
            &replacement,
            &authorization_path(),
            &permission_for_complete(mr_id),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "new current body has configured operation: {control}"
        );
        let mut wire = completion.clone();
        wire.as_object_mut().unwrap().remove("merge_request_id");
        wire["operation_id"] = json!("full-r2-retained-old-body-valid-approval-denied");
        let (status, error) = connected_authorization_post(
            &h.backend_url,
            &h.identity,
            &other_worker,
            &format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests/{mr_id}/complete"),
            &wire,
        )
        .await;
        // Explicit release revokes the old Subject source at signed ingress.
        // This is the real HTTP refusal, not the component admission callback's
        // 403, and not a missing Worker or invalid approval/target fallback.
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "retained old body source must be fenced: {error}"
        );
        assert_eq!(error["message"], "invalid runtime request proof");
    }
    // Exact current assignment CAS, not reuse of the old assignment receipt.
    let current = h
        .api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
        .unwrap()
        .remove(0);
    assert_eq!(
        current.assignment_id,
        moved["assignment"]["assignment_id"].as_str().unwrap()
    );
    let restored = ok(&h,"PUT",&format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),json!({
        "operation_id":"full-r2-explicit-return-assignment","expected_assignment_id":current.assignment_id,
        "principal":{"kind":"subject","subject_id":subject}})).await;
    assert_ne!(
        restored["assignment"]["assignment_id"],
        old_row["assignment_id"]
    );
    super::super::chat_tests::adopt_subject_policy(&h.api,&h.owner,&subject,
        r#"{ sets = ["conversation", "work-coordination"]; namespaces = [{ namespace = "ticket"; keys = ["ticket.assignment.assign"]; }, { namespace = "worker"; keys = ["worker.create", "worker.observe", "worker.messaging"]; }, { namespace = "merge_request"; keys = ["merge_request.read", "merge_request.readiness", "merge_request.complete"]; }]; exclude_namespaces = [{ namespace = "merge_request"; keys = ["merge_request.complete"]; }]; }"#,FULL_DELEGATION).await;
    completion_denied(
        &h,
        &owner_worker,
        completion,
        "full-r2-current-body-revoked-denied",
        false,
    )
    .await;
    super::super::chat_tests::adopt_subject_policy(&h.api, &h.owner, &subject, "{}", "{}").await;
    completion_denied(
        &h,
        &owner_worker,
        completion,
        "full-r2-current-body-empty-denied",
        false,
    )
    .await;
    // Canonical restoration does not resurrect any old child grant. No child is
    // asked to perform further effects: result/review/adoption already committed.
    super::super::chat_tests::adopt_subject_policy(
        &h.api,
        &h.owner,
        &subject,
        FULL_EXECUTION,
        FULL_DELEGATION,
    )
    .await;
    assert_eq!(
        serde_json::to_value(&mr_store.get_by_id(TEST_WORKSPACE_ID, mr_id).unwrap().thread)
            .unwrap(),
        before_events,
        "none of the refused valid-evidence completion attempts creates an event"
    );
    let continued = ok(&h,"POST",&format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations/{}/messages",conversation["id"].as_str().unwrap()),json!({
        "operation_id":"full-r2-continue-after-refusal-controls","text":format!("FULL_R2_COMPLETION_CONTINUE:{ticket}")})).await;
    assert_eq!(continued["delivery"]["state"], "accepted", "{continued}");
    let finished = tokio::time::timeout(LIMIT, async {
        loop {
            let value = events.recv().await.unwrap();
            if value["stage"] == "body_done" {
                break value;
            }
        }
    })
    .await
    .expect("current assigned Subject records typed completion/replay and handling");
    let final_mr = mr_store.get_by_id(TEST_WORKSPACE_ID, mr_id).unwrap();
    let merged: Vec<_> = final_mr
        .thread
        .iter()
        .filter_map(|event| match event {
            merge_request::MergeRequestThreadEvent::Merge(value) => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(
        merged.len(),
        1,
        "exact replay records no duplicate merge event"
    );
    assert_eq!(
        merged[0].event_id,
        finished["completion"]["event_id"].as_str().unwrap()
    );
    assert_eq!(merged[0].approved_source_ref, source);
    assert_eq!(
        merged[0].approval_event_id,
        ready["approval"]["event_id"].as_str().unwrap()
    );
    assert_eq!(merged[0].merged_by.worker_id, owner_worker.worker_id);
    let tracking = h
        .api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
        .unwrap()
        .remove(0);
    assert_eq!(
        tracking.assignment_id,
        restored["assignment"]["assignment_id"].as_str().unwrap()
    );
    assert_eq!(
        tracking.tracking.unwrap().status,
        ticket::TicketAssignmentTrackingStatus::Done
    );
    assert_eq!(
        h.api.authority.ticket(&ticket).unwrap().state,
        server_api::BrowserTicketState::Open
    );
    for root in [&coder_root, &adopt_root] {
        assert!(git(root, &["status", "--porcelain"]).is_empty());
    }

    assert_eq!(
        h.api.authority.ticket(&ticket).unwrap().content_digest,
        ready["requirements_digest"].as_str().unwrap()
    );
    let body_snapshot = h
        .factory
        .retained_session_snapshot(
            &worker_runtime::identity::WorkerRef::try_from(&owner_worker).unwrap(),
        )
        .unwrap();
    let incoming = notifications(&body_snapshot.snapshot);
    assert!(
        incoming
            .iter()
            .any(|message| message.contains("implementation-result:")
                && message.contains(ready["result_id"].as_str().unwrap())),
        "canonical incoming business notification must carry the same saved result ID"
    );
    let adoption_result = observed["adopter_done"]["receipt"]["result_id"]
        .as_str()
        .unwrap();
    assert!(
        incoming
            .iter()
            .any(|message| message.contains("implementation-result:")
                && message.contains(adoption_result)),
        "separate adopter reports its saved published-command result through the canonical notification"
    );
    // The old-body variant releases an unused body before it has Job-backed
    // Journal work. Replacing the long-running owner also requires actual Host
    // Job cleanup fences, which this harness's substituted Job executor cannot
    // certify; the test never forges those fences to force a replacement.
    let final_events = mr_store.get_by_id(TEST_WORKSPACE_ID, mr_id).unwrap().thread;
    assert_eq!(
        serde_json::to_value(&final_events).unwrap(),
        serde_json::to_value(&final_mr.thread).unwrap()
    );
    assert_eq!(
        git(
            h._dir.path(),
            &["rev-parse", &format!("refs/heads/{FULL_TARGET}")]
        ),
        source
    );
    assert_eq!(git(h._dir.path(), &["rev-parse", "HEAD"]), before);
}
