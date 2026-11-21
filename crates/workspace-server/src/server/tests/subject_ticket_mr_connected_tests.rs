//! Normal Server library tests for ordinary Subject MR coordination. The signed
//! Backend/Runtime, canonical policy/body/Profile/Flow, Workdirs/Git/provider,
//! independent builtin Reviewer, formal review endpoint and automatic completion are real.
//! By default model decisions (including Flow verification) are scripted, not
//! live-model or real Host integration evidence. The explicitly ignored live test
//! delegates Coder, Reviewer and verifier to the configured provider; Subject
//! setup/result collection remains scripted. Runtime directories must be isolated
//! as required by the shared connected harness.
//!
//! Operation assignments are frozen within the Subject's delegation ceiling;
//! Ticket ownership, Workdir attachments and trusted Reviewer capabilities remain
//! separate checks. The full scenario contrasts valid approval/published-target
//! evidence with authority refusals.
//! Approval wakes production publication against the Coder's isolated temp clone.
//! No Subject completion call or model-side target push drives the positive path.
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

// Creation is deliberately idle and Ticket-free. Observe the exact live
// connections through the public Tool, confirm the claim, then submit typed input.
fn assign_and_submit(
    request: &agen::llm_client::Request,
    prefix: &str,
    ticket: &str,
    row: &Value,
    spawned: &Value,
    spawn: &Value,
    segments: Value,
) -> Option<Vec<Event>> {
    assert_eq!(spawn["initial_submit"], json!([]));
    for removed in [
        "ticket_id",
        "orchestrator_assignment_id",
        "initial_request",
        "flow",
        "working_directory_id",
    ] {
        assert!(
            spawn.get(removed).is_none(),
            "general spawn must not carry {removed}"
        );
    }
    let list_id = format!("{prefix}-connections");
    let Some(listed) = success(request, &list_id) else {
        return Some(call(request, &list_id, "WorkerList", json!({})));
    };
    assert_eq!(
        listed["truncated"], false,
        "all known connections must be observable"
    );
    let workers: Vec<_> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            item["target"]["kind"] == "runtime_worker"
                && item["target"]["runtime_id"] == spawned["runtime_id"]
                && item["target"]["worker_id"] == spawned["worker_id"]
        })
        .collect();
    assert_eq!(
        workers.len(),
        1,
        "use the one Worker just created, not a replacement"
    );
    let worker = workers[0];
    let connections = worker["workdir_connections"].as_array().unwrap();
    let attachments = spawn["workdir_attachments"].as_array().unwrap();
    assert_eq!(connections.len(), attachments.len());
    let bindings: Vec<_> = connections.iter().map(|connection| {
        assert!(attachments.iter().any(|attachment| {
            attachment["alias"] == connection["alias"]
                && attachment["working_directory_id"] == connection["working_directory_id"]
        }), "only the selected Workdir attachments may be claimed: {connection}");
        assert!(!connection["connection_id"].as_str().unwrap().is_empty());
        json!({"alias":connection["alias"],"working_directory_id":connection["working_directory_id"],
            "connection_id":connection["connection_id"]})
    }).collect();
    let claim_id = format!("{prefix}-claim");
    let Some(claimed) = success(request, &claim_id) else {
        return Some(call(
            request,
            &claim_id,
            "TicketWorkerAssign",
            json!({
            "ticket":ticket,"runtime_id":spawned["runtime_id"],"worker_id":spawned["worker_id"],
            "orchestrator_assignment_id":row["assignment_id"],
            "operation_id":format!("{prefix}-claim-{ticket}"),"workdir_bindings":bindings}),
        ));
    };
    assert_eq!(claimed["ticket_id"], ticket);
    assert_eq!(claimed["assignment"]["role"], "worker");
    assert!(claimed["assignment"]["assignment_id"].is_string());
    assert_eq!(
        claimed["assignment"]["principal"],
        json!({"kind":"worker",
        "runtime_id":spawned["runtime_id"],"worker_id":spawned["worker_id"]})
    );
    let submit_id = format!("{prefix}-submit");
    let Some(accepted) = success(request, &submit_id) else {
        return Some(call(
            request,
            &submit_id,
            "WorkerSendInput",
            json!({
            "target":worker["target"],"content":"Implement the confirmed Ticket assignment",
            "segments":segments}),
        ));
    };
    // Input acceptance is distinct from the saved result inspected by the caller.
    assert_eq!(accepted["state"], "accepted", "{accepted}");
    None
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
        // Explicit post-assignment Flow input selected the actual builtin recipe.
        // Tool publication alone is not evidence of this Flow being active.
        let context = format!(
            "{}\n{}",
            request.system_prompt.as_deref().unwrap_or_default(),
            text(request)
        );
        assert!(
            context.contains("This is an optional code recipe"),
            "explicit coder-review implement instructions missing"
        );
        assert!(
            request
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
        assert!(
            request
                .tools
                .iter()
                .any(|tool| tool.name == "ShowMergeRequest")
        );
        for removed in ["CheckMergeRequestReadiness", "CompleteMergeRequest"] {
            assert!(!request.tools.iter().any(|tool| tool.name == removed));
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
            return call(
                request,
                "mr-body-spawn",
                "WorkerSpawn",
                input["spawn"].clone(),
            );
        };
        if let Some(events) = assign_and_submit(
            request,
            "mr-body",
            ticket,
            row,
            &spawned,
            &input["spawn"],
            json!([{"kind":"flow","selector":"builtin:coder-review"},
                {"kind":"text","content":format!("{CODER}{}",json!({"ticket":ticket}))}]),
        ) {
            return events;
        }
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
        assert_eq!(mr["state"], json!({"kind":"implementation"}));
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
        let readiness = &mr["summary"]["readiness"];
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
                "target":child,"query":"exit_code","tool_part":"output","limit":4}),
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
                "target":child,"entry_ref":entries[0]["entry_ref"],"mode":"full"}),
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
async fn ordinary_subject_explicit_coder_review_publishes_mr_and_preserves_unreviewed_wait() {
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
    // Configuration adoption leaves creation defaults unchanged. The child
    // selects operation keys; Ticket targets and attachments bound its Workdir.
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
    let assignment = standalone_assignment(
        r#"{ namespaces = [
            { namespace = "filesystem"; keys = ["filesystem.read", "filesystem.write"]; },
            { namespace = "bash"; keys = ["bash.execute"]; },
            { namespace = "ticket"; keys = ["ticket.read", "ticket.relation.read", "ticket.implementation_result.write"]; },
            { namespace = "merge_request"; keys = ["merge_request.open"]; }
        ]; }"#,
    );
    let spawn = json!({"runtime_id":CONNECTED_RUNTIME,"profile":"builtin:coder",
        "operation_id":"mr-connected-spawn","initial_submit":[],
        "workdir_attachments":[{"alias":"checkout","working_directory_id":wd}],
        "authorization":{"kind":"assignment","assignment":assignment}});
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

    // The Subject cannot turn an unreviewed MR into a completed one.
    let complete_path = format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests/{mr_id}/complete");
    let (status, error) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &original_body,
        &complete_path,
        &json!({"operation_id":"fabricated-completion","approval_event_id":"absent",
            "expected_owner_operation_id":mr.current_worker().unwrap().operation_id,
            "expected_source_ref":published}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "completion requires current Coder authority: {error}"
    );
    // Nor does a Backend advance invent approval or move the target.
    mr_publication::advance(&h.api, mr_id).unwrap();
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
        .filter(|e| e["kind"] == kind)
        .filter(|e| {
            !matches!(kind, "review" | "review_requested")
                || e["subject_ref"] == mr["source"]["ref"]
        });
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
    live: Option<Arc<dyn agen::llm_client::LlmClient>>,
    handoff: bool,
    scope_decision: bool,
    integrate_target: bool,
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
            "observe"
                if input["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("Resume existing MR") =>
            {
                (
                    "ShowMergeRequest",
                    "\"kind\":\"integration\"",
                    "worker_assignment",
                    "integrate",
                )
            }
            "observe" => (
                "ShowTicket",
                "full R2 normal requirement",
                "content_digest",
                "implement",
            ),
            "implement" => ("Bash", "full-r2-source-published", "exit_code", "review"),
            "review" => (
                "ShowMergeRequest",
                "approve",
                "ticket_content_digest",
                "integrate",
            ),
            "integrate"
                if input["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("integrated candidate") =>
            {
                (
                    "Bash",
                    "full-r2-integrated-source-published",
                    "exit_code",
                    "review",
                )
            }
            "integrate" => (
                "ShowMergeRequest",
                "\"kind\":\"completed\"",
                "merge_event_id",
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
        let integration_check = if input["integration"] == true {
            format!(
                "test \"$(cat TARGET.txt)\" = 'retain target work' && git merge-base --is-ancestor refs/remotes/origin/{FULL_TARGET} HEAD && "
            )
        } else {
            String::new()
        };
        call(
            request,
            "full-review-check",
            "Bash",
            json!({"target_workdir":"checkout","cwd":".","timeout":10000,
            "command":format!("{integration_check}test \"$(cat README.md)\" = 'full R2 independently reviewed candidate' && git diff --check && test -z \"$(git status --porcelain)\" && test \"$(git rev-parse HEAD)\" = '{source}' && test \"$(git ls-remote --exit-code --refs origin refs/heads/{FULL_SOURCE} | cut -f1)\" = '{source}' && git log -1 --oneline && printf 'full-r2-independent-validation\\n'")}),
        )
    }
    fn integrated_review(
        &self,
        request: &agen::llm_client::Request,
        ticket: &str,
        original: &Value,
    ) -> Option<Vec<Event>> {
        if let Some(entered) = success(request, "integration-reapproved") {
            assert_eq!(entered["entered_state"], "integrate");
            return None;
        }
        if summary_ok(request, "integration-reviewer-stop") {
            return Some(call(
                request,
                "integration-reapproved",
                "RequestFlowTransition",
                json!({"reason":"Fresh independent approval for the changed integration source is recorded; inspect current MR evidence"}),
            ));
        }
        if let Some(list) = success(request, "integration-reviewer-list") {
            let child = list["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["subject"]["name"] == "integration-reviewer")
                .unwrap();
            let Some(found) = success(request, "integration-review-search") else {
                return Some(call(
                    request,
                    "integration-review-search",
                    "SearchSessionEntries",
                    json!({"subject":child["subject"],"query":"Scripted independent Reviewer inspected","tool_part":"output","limit":4}),
                ));
            };
            let entry = found["entries"].as_array().unwrap().last().unwrap();
            let Some(read) = success(request, "integration-review-read") else {
                return Some(call(
                    request,
                    "integration-review-read",
                    "ReadSessionEntry",
                    json!({"subject":child["subject"],"entry_ref":entry["entry_ref"],"mode":"full"}),
                ));
            };
            let rendered = read["entries"][0]["text"].as_str().unwrap();
            let retained: Value =
                serde_json::from_str(rendered.split_once("\ncontent: ").unwrap().1).unwrap();
            let mr = success(request, "integration-verdict").unwrap();
            let mut canonical = event(&mr, "review").clone();
            canonical.as_object_mut().unwrap().remove("kind");
            assert_eq!(
                retained, canonical,
                "committed child verdict matches the exact integrated source"
            );
            return Some(call(
                request,
                "integration-reviewer-stop",
                "WorkerStop",
                json!({"subject":child["subject"],"reason":"Fresh formal integration-source verdict observed"}),
            ));
        }
        if let Some(mr) = success(request, "integration-verdict") {
            let requirements = success(request, "full-coder-requirements").unwrap();
            assert_approved(&mr, &requirements);
            assert_ne!(
                mr["source"]["ref"], original["source"]["ref"],
                "changed source must have its own approval"
            );
            return Some(call(
                request,
                "integration-reviewer-list",
                "WorkerList",
                json!({}),
            ));
        }
        if summary_ok(request, "integration-spawn-reviewer") {
            if !text(request).contains("SubWorker `integration-reviewer` lifecycle RunFinished") {
                return Some(done());
            }
            return Some(call(
                request,
                "integration-verdict",
                "ShowMergeRequest",
                json!({"merge_request_id":original["merge_request_id"],"view":"detail","limit":50}),
            ));
        }
        if let Some(entered) = success(request, "integration-flow-review") {
            assert_eq!(entered["entered_state"], "review");
            return Some(call(
                request,
                "integration-spawn-reviewer",
                "SubWorkerSpawn",
                json!({
                    "name":"integration-reviewer","profile":"builtin:reviewer","command":true,
                    "scope":[{"target_workdir":"checkout","target":".","permission":"write"}],
                    "review":{"ticket_id":ticket,"merge_request_id":original["merge_request_id"]},
                    "task":format!("{REVIEWER}{}",json!({"ticket":ticket,"merge_request_id":original["merge_request_id"],"integration":true}))
                }),
            ));
        }
        if let Some(mr) = success(request, "integration-source") {
            assert_ne!(mr["source"]["ref"], original["source"]["ref"]);
            assert_ne!(
                mr["summary"]["review_status"], "approved",
                "old approval cannot transfer to merged source"
            );
            return Some(call(
                request,
                "integration-flow-review",
                "RequestFlowTransition",
                json!({"reason":"The integrated candidate is committed, validated and source published; changed source needs fresh independent review"}),
            ));
        }
        if checked_command(request, "integration-merge").is_some() {
            return Some(call(
                request,
                "integration-source",
                "ShowMergeRequest",
                json!({"merge_request_id":original["merge_request_id"],"view":"detail","limit":50}),
            ));
        }
        let Some(mr) = success(request, "integration-before") else {
            return Some(call(
                request,
                "integration-before",
                "ShowMergeRequest",
                json!({"merge_request_id":original["merge_request_id"],"view":"detail","limit":50}),
            ));
        };
        assert_eq!(mr["state"]["kind"], "integration");
        assert!(
            mr["thread"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"] == "comment"
                    && e["body"].as_str().unwrap_or("").contains("publication")),
            "publication blocker must be visible: {mr}"
        );
        Some(call(
            request,
            "integration-merge",
            "Bash",
            json!({"target_workdir":"checkout","cwd":".","timeout":10000,
                "command":format!("git fetch origin {FULL_TARGET} && (git -c user.name=Test -c user.email=test@example.invalid merge --no-commit --no-ff FETCH_HEAD; test $? -eq 1) && test -n \"$(git ls-files -u README.md)\" && printf 'full R2 independently reviewed candidate\\n' > README.md && test \"$(cat TARGET.txt)\" = 'retain target work' && git add README.md TARGET.txt && git -c user.name=Test -c user.email=test@example.invalid commit -m 'fix: resolve integration target conflict' && git diff --check && test -z \"$(git status --porcelain)\" && git merge-base --is-ancestor FETCH_HEAD HEAD && git push origin HEAD:refs/heads/{FULL_SOURCE} && test \"$(git rev-parse HEAD)\" = \"$(git ls-remote --refs origin refs/heads/{FULL_SOURCE} | cut -f1)\" && printf 'full-r2-integrated-source-published\\n'")
            }),
        ))
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
            request
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
            "actual explicit builtin coder-review Flow missing"
        );
        if let Some(existing) = input["existing_mr"].as_str() {
            if success(request, "full-coder-flow-approved").is_none() {
                if success(request, "full-coder-verdict").is_none() {
                    return call(
                        request,
                        "full-coder-verdict",
                        "ShowMergeRequest",
                        json!({"merge_request_id":existing,"view":"detail","limit":50}),
                    );
                }
                return call(
                    request,
                    "full-coder-flow-approved",
                    "RequestFlowTransition",
                    json!({"reason":"Resume existing MR from saved Integration phase/current worker/source/review; do not redo implementation"}),
                );
            }
        } else if success(request, "full-coder-flow-implement").is_none() {
            return call(
                request,
                "full-coder-flow-implement",
                "RequestFlowTransition",
                json!({"reason":"Current Ticket requirements and assigned Workdir permit new implementation; there is no existing MR in this scenario"}),
            );
        }
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
        if let Some(completed) = success(request, "full-coder-flow-completed") {
            assert_eq!(completed["entered_state"], "complete", "{completed}");
            let mr = success(request, "full-coder-integration-resumed")
                .or_else(|| success(request, "full-coder-integration-observed"))
                .unwrap();
            assert_eq!(mr["state"]["kind"], "completed", "{mr}");
            assert!(
                mr["thread"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["kind"] == "merge")
            );
            assert_approved(&mr, &requirements);
            return call(
                request,
                "full-coder-result",
                "TicketImplementationResult",
                json!({"ticket":ticket,"kind":"result",
                "body":"full-r2 reviewed result: real edit/commit/validation/non-force source publication; actual direct builtin Reviewer formal approval; approval triggers Backend publication without a Subject stamp. Scripted model choices, not live-model behavior.",
                "merge_request":{"merge_request_id":mr["merge_request_id"],"subject_ref":mr["source"]["ref"]}}),
            );
        }
        if let Some(approved) = success(request, "full-coder-flow-approved") {
            assert_eq!(approved["entered_state"], "integrate", "{approved}");
            let original = success(request, "full-coder-verdict").unwrap();
            if self.scope_decision {
                if let Some(receipt) = success(request, "scope-result") {
                    self.emit_once(
                        "coder_done",
                        json!({"stage":"coder_done","receipt":receipt}),
                    )
                    .await;
                    return done();
                }
                if success(request, "scope-question").is_some() {
                    return call(
                        request,
                        "scope-result",
                        "TicketImplementationResult",
                        json!({"ticket":ticket,"kind":"blocker","body":"Target README conflict changes the adopted scope; Subject decision required. No integration mutation or target push performed.","merge_request":{"merge_request_id":original["merge_request_id"],"subject_ref":original["source"]["ref"]}}),
                    );
                }
                return call(
                    request,
                    "scope-question",
                    "CommentMergeRequest",
                    json!({"merge_request_id":original["merge_request_id"],"expected_owner_operation_id":original["worker_assignment"]["operation_id"],"operation_id":"scope-question","body":"Scope decision required: target README conflicts with requested source. Preserve both refs until Subject chooses the intended requirement; no silent target/scope change."}),
                );
            }
            if self.handoff && input["existing_mr"].is_null() {
                self.emit_once("handoff", json!({"stage":"handoff","mr":original}))
                    .await;
                return done();
            }
            if self.integrate_target {
                if let Some(events) = self.integrated_review(request, ticket, &original) {
                    return events;
                }
            }
            if text(request).contains("full-r2-manual-recovery")
                && success(request, "manual-complete").is_none()
            {
                if checked_command(request, "manual-push").is_none() {
                    return call(
                        request,
                        "manual-push",
                        "Bash",
                        json!({"target_workdir":"checkout","cwd":".","timeout":10000,
                        "command":format!("git push origin HEAD:refs/heads/{FULL_TARGET} && later=$(printf 'later target publication\\n' | git -c user.name=Test -c user.email=test@example.invalid commit-tree HEAD^{{tree}} -p HEAD) && git push origin $later:refs/heads/{FULL_TARGET}")}),
                    );
                }
                return call(
                    request,
                    "manual-complete",
                    "CompleteMergeRequest",
                    json!({
                    "merge_request_id":original["merge_request_id"],"operation_id":"manual-completion",
                    "expected_owner_operation_id":original["worker_assignment"]["operation_id"],
                    "approval_event_id":event(&original,"review")["event_id"],
                    "expected_source_ref":original["source"]["ref"]}),
                );
            }
            let resumed = text(request).contains("full-r2-publication-resume");
            let id = if resumed {
                "full-coder-integration-resumed"
            } else {
                "full-coder-integration-observed"
            };
            let Some(mr) = success(request, id) else {
                return call(
                    request,
                    id,
                    "ShowMergeRequest",
                    json!({"merge_request_id":original["merge_request_id"],"view":"detail","limit":20}),
                );
            };
            assert_approved(&mr, &requirements);
            if mr["state"]["kind"] == "completed" {
                return call(
                    request,
                    "full-coder-flow-completed",
                    "RequestFlowTransition",
                    json!({"reason":"Fresh ShowMergeRequest confirms Completed and saved publication evidence for exact approved source; approval alone was not treated as completion"}),
                );
            }
            assert_eq!(mr["state"]["kind"], "integration", "{mr}");
            self.emit_once(
                "publication_wait",
                json!({"stage":"publication_wait","mr":mr}),
            )
            .await;
            // No repeated reads or model-side push: resume only on explicit new input.
            return done();
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
                .find(|item| item["target"]["kind"] == "sub_worker")
                .unwrap();
            return call(
                request,
                "full-coder-reviewer-stop",
                "WorkerStop",
                json!({"target":child["target"],"reason":"Exact-source formal verdict and committed child evidence inspected"}),
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
                .find(|item| item["target"]["kind"] == "sub_worker")
                .unwrap();
            let formal = entries.last().unwrap();
            return call(
                request,
                "full-coder-review-read",
                "ReadSessionEntry",
                json!({"target":child["target"],"entry_ref":formal["entry_ref"],"mode":"full"}),
            );
        }
        if let Some(list) = success(request, "full-coder-reviewer-list") {
            let children: Vec<_> = list["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["target"]["kind"] == "sub_worker")
                .collect();
            assert_eq!(children.len(), 1, "one independent direct child");
            assert_eq!(children[0]["relation"], "direct_child");
            assert_eq!(children[0]["target"]["name"], REVIEWER_NAME);
            return call(
                request,
                "full-coder-review-search",
                "SearchSessionEntries",
                json!({
                "target":children[0]["target"],"query":"Scripted independent Reviewer inspected","tool_part":"output","limit":4}),
            );
        }
        if let Some(mr) = success(request, "full-coder-verdict") {
            assert_approved(&mr, &requirements);
            return call(request, "full-coder-reviewer-list", "WorkerList", json!({}));
        }
        if summary_ok(request, "full-coder-spawn-reviewer") {
            // Only the real child's run-finished lifecycle notification wakes
            // this branch. No shared test channel supplies a verdict to the Coder.
            let incoming = text(request);
            if !incoming.contains(&format!(
                "SubWorker `{REVIEWER_NAME}` lifecycle RunFinished"
            )) {
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
            return call(
                request,
                "full-body-spawn-coder",
                "WorkerSpawn",
                input["spawn"].clone(),
            );
        };
        if let Some(events) = assign_and_submit(
            request,
            "full-body",
            ticket,
            row,
            &coder,
            &input["spawn"],
            json!([{"kind":"flow","selector":"builtin:coder-review"},
                {"kind":"text","content":format!("{FULL_CODER}{}",json!({"ticket":ticket}))},
                {"kind":"text","content":format!("Read Ticket {ticket} and implement its requirements in Workdir checkout. The source branch is {FULL_SOURCE}, target {FULL_TARGET}, repository test-repository. Follow the selected coder-review Flow through independent review and saved Backend publication/completion; report the result with TicketImplementationResult. These refs and all resources are isolated test fixtures, not the Host's develop branch. Do not request another Subject approval or manually push target. Do not wait by polling; return if blocked.")}]),
        ) {
            return events;
        }
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
        assert_eq!(
            saved["request"]["kind"],
            if self.scope_decision {
                "blocker"
            } else {
                "result"
            }
        );
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
        self.emit_once("body_result", json!({"stage":"body_result", "mr":mr,
            "coder":{"kind":"runtime_worker","runtime_id":mr["worker_assignment"]["worker"]["runtime_id"],"worker_id":mr["worker_assignment"]["worker"]["worker_id"]},
            "result_id":result_id,"requirements_digest":requirements["content_digest"]})).await;
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
        if let Some(live) = &self.live {
            // Only ordinary Subject setup/result collection is scripted. The Coder,
            // actual Reviewer child and actual Flow verifier use the real provider.
            if marker(&request, FULL_CODER).is_some()
                || request.tools.iter().any(|t| {
                    matches!(
                        t.name.as_str(),
                        "ReviewMergeRequest" | "FinishFlowVerification"
                    )
                })
            {
                let role = if request
                    .tools
                    .iter()
                    .any(|t| t.name == "FinishFlowVerification")
                {
                    "verifier"
                } else if marker(&request, FULL_CODER).is_some() {
                    "coder"
                } else {
                    "reviewer"
                };
                let results: Vec<_> = request
                    .items
                    .iter()
                    .rev()
                    .filter_map(|item| match item {
                        Item::ToolResult {
                            summary, is_error, ..
                        } => Some(format!(
                            "{is_error}:{}",
                            summary.chars().take(300).collect::<String>()
                        )),
                        _ => None,
                    })
                    .take(3)
                    .collect();
                let calls: Vec<_> = request.items.iter().rev().filter_map(|item| match item {
                    Item::ToolCall { name, arguments, .. } if matches!(name.as_str(), "ShowTicket" | "ShowMergeRequest" | "SubWorkerSpawn") => {
                        let parsed: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
                        Some(json!({"tool":name,"id":parsed["id"],"mr":parsed["merge_request_id"],"review":parsed["review"]}))
                    }, _ => None
                }).take(2).collect();
                eprintln!("live role={role} recent_results={results:?} references={calls:?}");
                return live.stream(request).await;
            }
        }
        if let Some(Item::ToolResult {
            call_id,
            is_error,
            summary,
            ..
        }) = request
            .items
            .iter()
            .rev()
            .find(|item| matches!(item, Item::ToolResult { .. }))
        {
            eprintln!(
                "MR scenario tool result: {call_id} error={is_error} {}",
                summary.chars().take(400).collect::<String>()
            );
        }
        let events = if request
            .tools
            .iter()
            .any(|tool| tool.name == "FinishFlowVerification")
        {
            self.verifier(&request)
        } else if let Some(input) = marker(&request, REVIEWER) {
            self.reviewer(&request, input).await
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
async fn body_idle_now(h: &ConnectedWorkerHarness, worker: &RuntimeWorkerRef) {
    let mut observed = h.runtime.subscribe_worker_observation().unwrap();
    let source = worker_runtime::identity::WorkerRef::try_from(worker).unwrap();
    tokio::time::timeout(LIMIT, async {
        loop {
            // The Backend projection can still say idle after a notification
            // started a new turn. Policy controls require the execution owner's
            // current snapshot, not a delayed projection or an older idle event.
            if matches!(
                h.runtime.worker_observation_snapshot(&source).unwrap(),
                protocol::Event::Snapshot { state, .. }
                    if state.state == protocol::WorkerState::Idle
            ) {
                break;
            }
            let event = observed.recv().await.expect("real body observation");
            if event.worker_ref.worker_id.to_string() != worker.worker_id {
                continue;
            }
            match event.payload {
                protocol::Event::WorkerState { .. } => {}
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
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ordinary_subject_independent_approval_automatically_publishes_and_completion_requires_evidence()
 {
    full_mr_scenario(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ordinary_subject_mr_valid_approval_rejects_retained_old_body_after_explicit_replacement() {
    full_mr_scenario(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ordinary_coder_integrates_conflicting_target_and_reviews_changed_source_before_auto_publication()
 {
    full_mr_scenario(false, true).await;
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

async fn full_mr_scenario(replace_body: bool, integrate_target: bool) {
    full_mr_run(replace_body, integrate_target, None, false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn stopped_coder_continues_on_another_worker_without_replacing_ticket_assignment() {
    full_mr_run(false, true, None, true, false, false).await;
}

// Explicit provider access only; the normal suite never contacts a model.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn integration_scope_conflict_returns_question_without_mutation_or_publication() {
    full_mr_run(false, true, None, false, true, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "opt-in live Codex OAuth, isolated Git/Backend/Runtime; requires existing authorized provider login"]
async fn live_coder_reviewer_and_flow_verifier_complete_isolated_mr() {
    let manifest: manifest::ModelManifest =
        serde_json::from_value(json!({"ref":"codex-oauth/gpt-6.1-sol"})).unwrap();
    let live = worker::model_client::build_client(&manifest)
        .expect("configured real model client; no scripted fallback");
    full_mr_run(false, false, Some(Arc::from(live)), false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn rejected_publication_recovers_with_ordinary_git_and_verified_completion() {
    full_mr_run(false, false, None, false, false, true).await;
}

async fn full_mr_run(
    replace_body: bool,
    integrate_target: bool,
    live: Option<Arc<dyn agen::llm_client::LlmClient>>,
    handoff: bool,
    scope_decision: bool,
    manual_recovery: bool,
) {
    crate::restore_test_environment::require_restore_test_environment();
    let is_live = live.is_some();
    let deadline = if is_live {
        Duration::from_secs(480)
    } else {
        LIMIT
    };
    let (stages, mut events) = tokio::sync::mpsc::channel(12);
    let h = ConnectedWorkerHarness::start(FullModel {
        live,
        handoff,
        scope_decision,
        integrate_target,
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
    let reject_hook = h._dir.path().join(".git/hooks/update");
    if manual_recovery {
        use std::os::unix::fs::PermissionsExt;
        fs::write(&reject_hook, format!("#!/bin/sh\nif [ \"$1\" = refs/heads/{FULL_TARGET} ]; then echo explicit-test-rejection >&2; exit 1; fi\nexit 0\n")).unwrap();
        fs::set_permissions(&reject_hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (mut coder_wd, mut coder_root) = temp_workdir(&h, "full-r2-coder-workdir").await;
    let target_before = if integrate_target {
        // Independent target publication after the source base was selected.
        git(&coder_root, &["switch", "-c", "target-fixture"]);
        std::fs::write(coder_root.join("README.md"), "conflicting target change\n").unwrap();
        std::fs::write(coder_root.join("TARGET.txt"), "retain target work\n").unwrap();
        git(&coder_root, &["add", "README.md", "TARGET.txt"]);
        git(
            &coder_root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "feat: independent target change",
            ],
        );
        git(
            &coder_root,
            &["push", "origin", &format!("HEAD:refs/heads/{FULL_TARGET}")],
        );
        git(&coder_root, &["rev-parse", "HEAD"])
    } else {
        before.clone()
    };
    git(&coder_root, &["switch", "-c", FULL_SOURCE, &before]);
    let (subject, body) =
        normal_subject(&h, "full-r2-owner", FULL_EXECUTION, FULL_DELEGATION).await;
    let (other,other_body) = normal_subject(&h,"full-r2-other",
        r#"{ sets = ["conversation", "work-coordination"]; namespaces = [{ namespace = "merge_request"; keys = ["merge_request.read"]; }]; }"#,"{}").await;
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
    ticket.body = "full R2 normal requirement: change README to 'full R2 independently reviewed candidate', commit, validate, publish only the source and use the actual direct builtin Reviewer for a formal exact-source verdict. Approve triggers automatic publication in the Coder temporary clone; no Subject completion stamp is needed. Do not touch the real Host target; keep Ticket conclusion separate.".into();
    if scope_decision {
        ticket.body = format!("{} If target README conflicts, the intended scope is unresolved: record the question on the MR and return a blocker to the Subject without resolving it or changing refs.", ticket.body.as_str()).into();
    }
    set_test_ticket_target(&mut ticket, "test-repository", FULL_TARGET);
    let ticket = backend.create(ticket).unwrap().id;
    let _assignment = ok(&h,"PUT",&format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),json!({
        "operation_id":"full-r2-assign","expected_assignment_id":null,"principal":{"kind":"subject","subject_id":subject}})).await;
    let conversation = ok(
        &h,
        "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations"),
        json!({
        "operation_id":"full-r2-chat","subject_id":subject}),
    )
    .await;
    // Internal Reviewer access uses the parent's current operation ceiling,
    // scoped Workdir broker and trusted exact-source review capability. The
    // public creation contract does not accept a separate redelegation vector.
    let coder_assignment = standalone_assignment(
        r#"{ namespaces = [
            { namespace = "filesystem"; keys = ["filesystem.read", "filesystem.write"]; },
            { namespace = "bash"; keys = ["bash.execute"]; },
            { namespace = "ticket"; keys = ["ticket.read", "ticket.relation.read", "ticket.implementation_result.write"]; },
            { namespace = "merge_request"; keys = ["merge_request.open", "merge_request.read", "merge_request.readiness", "merge_request.review", "merge_request.publish", "merge_request.comment", "merge_request.complete"]; },
            { namespace = "worker"; keys = ["worker.list"]; }
        ]; }"#,
    );
    let spawn = json!({"runtime_id":CONNECTED_RUNTIME,"profile":"builtin:coder",
        "operation_id":"full-r2-spawn-coder","initial_submit":[],
        "workdir_attachments":[{"alias":"checkout","working_directory_id":coder_wd}],
        "authorization":{"kind":"assignment","assignment":coder_assignment}});
    assert!(
        spawn.get("flow").is_none(),
        "Flow belongs to post-assignment input"
    );
    let input = json!({"subject":subject,"ticket":ticket,"spawn":spawn});
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
    tokio::time::timeout(deadline, async {
        let required = if is_live { vec!["body_result"] } else { vec!["coder_done", "body_result"] };
        while !required
            .iter()
            .all(|stage| observed.contains_key(*stage))
        {
            let value = events.recv().await.expect("real model stages");
            eprintln!("MR scenario stage: {}", value["stage"]);
            if value["stage"] == "handoff" {
                let mr_id = value["mr"]["merge_request_id"].as_str().unwrap();
                let old = &value["mr"]["worker_assignment"]["worker"];
                let old_worker = RuntimeWorkerRef::new(old["runtime_id"].as_str().unwrap(), old["worker_id"].as_str().unwrap());
                body_idle_now(&h, &old_worker).await;
                let retained = h.api.store.get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket).unwrap();
                ok(&h, "POST", &format!("/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/stop", old_worker.runtime_id, old_worker.worker_id), json!({"reason":"test explicit stopped-Coder handoff"})).await;
                let (new_wd, new_root) = temp_workdir(&h, "continuation-workdir").await;
                git(&new_root, &["fetch", "origin", FULL_SOURCE]);
                git(&new_root, &["switch", "-c", FULL_SOURCE, "FETCH_HEAD"]);
                let created = ok(&h, "POST", &format!("/api/w/{TEST_WORKSPACE_ID}/workers"), json!({
                    "runtime_id":CONNECTED_RUNTIME,"display_name":"integration-continuation","feature_connections":{},"profile":"builtin:coder","control_operation_id":"continuation-worker","initial_submit":[],
                    "workdir_attachments":[{"alias":"checkout","working_directory_id":new_wd}],"authorization":input["spawn"]["authorization"]
                })).await;
                let new_worker = json!({"runtime_id":created["runtime_id"],"worker_id":created["worker_id"]});
                let path = format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests/{mr_id}/worker");
                let bytes = serde_json::to_vec(&json!({"operation_id":"continuation-owner","expected_operation_id":value["mr"]["worker_assignment"]["operation_id"],"worker":new_worker})).unwrap();
                let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&h.identity)
                    .issue(&h.backend_url, TEST_WORKSPACE_ID, Some(&owner_worker.worker_id), worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION, "PUT", &path, &bytes, i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap(), 30).unwrap();
                let (status, assigned) = response(&h.api, Request::builder().method("PUT").uri(&path).header(CONTENT_TYPE, "application/json").header(worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER, proof).body(Body::from(bytes)).unwrap()).await;
                assert_eq!(status, StatusCode::OK, "{assigned}");
                assert_eq!(serde_json::to_value(h.api.store.get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket).unwrap()).unwrap(), serde_json::to_value(retained).unwrap());
                coder_wd = new_wd;
                coder_root = new_root;
                ok(&h, "POST", &format!("/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/input", created["runtime_id"].as_str().unwrap(), created["worker_id"].as_str().unwrap()), json!({"content":"Continue existing MR integration","segments":[{"kind":"flow","selector":"builtin:coder-review"},{"kind":"text","content":format!("{FULL_CODER}{}", json!({"ticket":ticket,"existing_mr":mr_id}))}]})).await;
            }
            if value["stage"] == "publication_wait" {
                let mr_id = value["mr"]["merge_request_id"].as_str().unwrap();
                let worker = &value["mr"]["worker_assignment"]["worker"];
                let coder = RuntimeWorkerRef::new(worker["runtime_id"].as_str().unwrap(), worker["worker_id"].as_str().unwrap());
                body_idle_now(&h, &coder).await;
                if manual_recovery {
                    let mr_id = value["mr"]["merge_request_id"].as_str().unwrap();
                    let source = value["mr"]["source"]["ref"].as_str().unwrap();
                    let store = merge_request_store(&h.api, TEST_WORKSPACE_ID).unwrap();
                    let mut ticks = tokio::time::interval(Duration::from_millis(50));
                    let problem = loop {
                        ticks.tick().await;
                        let mr = store.get_by_id(TEST_WORKSPACE_ID, mr_id).unwrap();
                        assert!(!mr.state.is_completed());
                        if let Some(c) = mr.thread.iter().find_map(|e| match e {
                            merge_request::MergeRequestThreadEvent::Comment(c) if c.body.contains("Automatic Git publication did not complete") => Some(c), _ => None,
                        }) { break c.clone(); }
                    };
                    eprintln!("MR scenario rejected publication comment observed");
                    let notice = format!("mr-event:{mr_id}:{}", problem.event_id);
                    let delivered = tokio::time::timeout(Duration::from_secs(15), async {
                        loop {
                            ticks.tick().await;
                            let snapshot = h.factory.retained_session_snapshot(&worker_runtime::identity::WorkerRef::try_from(&owner_worker).unwrap()).unwrap();
                            if notifications(&snapshot.snapshot).iter().any(|n| n.contains(&notice)) { break; }
                        }
                    }).await;
                    assert!(delivered.is_ok(), "publication problem notification missing: {:?}", h.api.store.notification_diagnostics(TEST_WORKSPACE_ID, &server_api::NotificationDiagnosticsQuery { source_event_id: Some(notice.clone()), ..Default::default() }).unwrap());
                    let path = format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests/{mr_id}/complete");
                    let completion = json!({"operation_id":"manual-completion", "expected_owner_operation_id":value["mr"]["worker_assignment"]["operation_id"], "approval_event_id":event(&value["mr"],"review")["event_id"], "expected_source_ref":source});
                    for bad in [completion.clone(), { let mut bad = completion.clone(); bad["expected_owner_operation_id"] = json!("stale-owner"); bad }, { let mut bad = completion.clone(); bad["approval_event_id"] = json!("other-MR-approval"); bad }] {
                        let (status, error) = connected_authorization_post(&h.backend_url, &h.identity, &coder, &path, &bad).await;
                        assert!(status.is_client_error(), "unpublished/stale/foreign approval must be rejected: {status} {error}");
                        assert!(!store.get_by_id(TEST_WORKSPACE_ID, mr_id).unwrap().state.is_completed());
                    }
                    assert_eq!(git(h._dir.path(), &["rev-parse", &format!("refs/heads/{FULL_TARGET}")]), before, "Complete never pushed");
                    fs::remove_file(&reject_hook).unwrap();
                    // Explicit test-driver recovery direction after actual Subject delivery;
                    // the Coder uses production Bash and Complete tools, not fixture inserts.
                    ok(&h, "POST", &format!("/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/input", coder.runtime_id, coder.worker_id),
                        json!({"content":"full-r2-manual-recovery full-r2-publication-resume: rejection cause removed; use ordinary non-force Git push then CompleteMergeRequest, reuse unchanged approval"})).await;
                    observed.insert(value["stage"].as_str().unwrap().to_owned(), value);
                    continue;
                }
                // Test-driver observation of the real background result, not model polling.
                let store = merge_request_store(&h.api, TEST_WORKSPACE_ID).unwrap();
                let mut ticks = tokio::time::interval(Duration::from_millis(50));
                loop {
                    ticks.tick().await;
                    if store.get_by_id(TEST_WORKSPACE_ID, mr_id).unwrap().state.is_completed() { break; }
                }
                ok(&h, "POST", &format!("/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/input", coder.runtime_id, coder.worker_id),
                    json!({"content":"full-r2-publication-resume: reread current MR and permissions, then report only saved completion evidence","segments":[{"kind":"text","content":"full-r2-publication-resume: reread current MR and permissions, then report only saved completion evidence"}]})).await;
            }
            observed.insert(value["stage"].as_str().unwrap().to_owned(), value);
        }
    })
    .await
    .expect("independent review and saved result reach the Subject");
    let ready = &observed["body_result"];
    let mr_id = ready["mr"]["merge_request_id"].as_str().unwrap();
    let source = ready["mr"]["source"]["ref"].as_str().unwrap();
    let mr_store = merge_request_store(&h.api, TEST_WORKSPACE_ID).unwrap();
    if scope_decision {
        let mr = mr_store.get_by_id(TEST_WORKSPACE_ID, mr_id).unwrap();
        assert!(matches!(
            mr.state,
            merge_request::MergeRequestState::Integration { .. }
        ));
        assert!(mr.thread.iter().any(|e| matches!(e, merge_request::MergeRequestThreadEvent::Comment(c) if c.body.contains("Scope decision required"))));
        assert!(
            mr_store
                .publications(TEST_WORKSPACE_ID, mr_id)
                .unwrap()
                .iter()
                .all(|p| !matches!(
                    p.outcome,
                    merge_request::PublicationOutcome::Published { .. }
                ))
        );
        assert_eq!(
            git(
                h._dir.path(),
                &["rev-parse", &format!("refs/heads/{FULL_TARGET}")]
            ),
            target_before
        );
        assert_eq!(git(&coder_root, &["rev-parse", "HEAD"]), source);
        assert!(git(&coder_root, &["status", "--porcelain"]).is_empty());
        return;
    }
    let final_mr = tokio::time::timeout(LIMIT, async {
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        loop {
            interval.tick().await;
            let mr = mr_store.get_by_id(TEST_WORKSPACE_ID, mr_id).unwrap();
            if mr.state.is_completed() {
                break mr;
            }
        }
    })
    .await
    .expect("approve must drive actual signed Runtime publication and MR completion");
    assert!(
        mr_store
            .publications(TEST_WORKSPACE_ID, mr_id)
            .unwrap()
            .is_empty(),
        "new publication writes no execution ledger"
    );
    let reviews: Vec<_> = final_mr
        .thread
        .iter()
        .filter_map(|e| match e {
            merge_request::MergeRequestThreadEvent::Review(r) => Some(r),
            _ => None,
        })
        .collect();
    assert_eq!(
        reviews.len(),
        if integrate_target { 2 } else { 1 },
        "unchanged source has no redundant review; changed source has a new verdict"
    );
    if integrate_target {
        assert_ne!(reviews[0].subject_ref, reviews[1].subject_ref);
        assert_eq!(reviews[1].subject_ref, source);
        assert_eq!(
            std::fs::read_to_string(coder_root.join("TARGET.txt")).unwrap(),
            "retain target work\n"
        );
        git(
            &coder_root,
            &["merge-base", "--is-ancestor", &target_before, source],
        );
    }
    let coder = RuntimeWorkerRef::new(
        ready["coder"]["runtime_id"].as_str().unwrap(),
        ready["coder"]["worker_id"].as_str().unwrap(),
    );
    let merges: Vec<_> = final_mr
        .thread
        .iter()
        .filter_map(|e| match e {
            merge_request::MergeRequestThreadEvent::Merge(e) => Some(e),
            _ => None,
        })
        .collect();
    assert_eq!(merges.len(), 1);
    assert_eq!(merges[0].merged_by.worker_id, coder.worker_id);
    assert_eq!(merges[0].approved_source_ref, source);
    assert_eq!(git(&coder_root, &["rev-parse", "HEAD"]), source);
    let published_target = git(
        h._dir.path(),
        &["rev-parse", &format!("refs/heads/{FULL_TARGET}")],
    );
    assert_eq!(merges[0].target_ref_after, published_target);
    if manual_recovery {
        assert_ne!(
            published_target, source,
            "later publication must not prevent delayed completion"
        );
        assert_eq!(
            merges[0].target_ref_before, published_target,
            "manual recording observes no ref update itself"
        );
        git(
            h._dir.path(),
            &["merge-base", "--is-ancestor", source, &published_target],
        );
    } else {
        assert_eq!(published_target, source);
    }
    assert_eq!(git(h._dir.path(), &["rev-parse", "HEAD"]), before);
    assert!(git(&coder_root, &["status", "--porcelain"]).is_empty());
    body_idle_now(&h, &coder).await;
    body_idle_now(&h, &owner_worker).await;
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests/{mr_id}/complete");
    let recorder = replacement.as_ref().unwrap_or(&other_worker);
    let completion = json!({"operation_id":if manual_recovery {"manual-completion"} else {"different-completion"},
        "expected_owner_operation_id":final_mr.current_worker().unwrap().operation_id,
        "approval_event_id":merges[0].approval_event_id,"expected_source_ref":source});
    for worker in [&owner_worker, &coder, recorder] {
        let (status, result) =
            connected_authorization_post(&h.backend_url, &h.identity, worker, &path, &completion)
                .await;
        if manual_recovery && worker == &coder {
            assert_eq!(status, StatusCode::OK, "exact completion replay: {result}");
            assert_eq!(result["event_id"], merges[0].event_id);
        } else {
            assert!(
                status.is_client_error(),
                "no new completion or Subject stamp: {status} {result}"
            );
        }
    }
    super::super::chat_tests::adopt_subject_policy(&h.api, &h.owner, &subject, "{}", "{}").await;
    if manual_recovery {
        let (status, error) =
            connected_authorization_post(&h.backend_url, &h.identity, &coder, &path, &completion)
                .await;
        assert!(
            status.is_client_error(),
            "revoked authority must reject even replay: {status} {error}"
        );
    }
    if replace_body {
        let (status, error) = connected_authorization_post(
            &h.backend_url,
            &h.identity,
            &other_worker,
            &authorization_path(),
            &json!({"operation":"merge_request.read","resource":{
                "kind":"merge_request","workspace_id":TEST_WORKSPACE_ID,"merge_request_id":mr_id}}),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "released old body is fenced: {error}"
        );
    }
    assert_eq!(
        mr_store.get_by_id(TEST_WORKSPACE_ID, mr_id).unwrap().thread,
        final_mr.thread
    );
    assert_eq!(
        h.api.authority.ticket(&ticket).unwrap().state,
        server_api::BrowserTicketState::Open
    );
    assert_eq!(
        h.api.authority.ticket(&ticket).unwrap().content_digest,
        ready["requirements_digest"].as_str().unwrap()
    );
    if !is_live {
        assert_eq!(
            observed["coder_done"]["receipt"]["result_id"],
            ready["result_id"]
        );
    } else {
        eprintln!(
            "live model=codex-oauth/gpt-6.1-sol; Coder/Reviewer/verifier=live; Subject=scripted; confirmed_publications=1; source={source}"
        );
    }
}
