// Real router response bytes -> production Web parser/operation/summary. The
// sibling lifecycle suite owns delivery; this bridge replaces only the Runtime
// execution boundary and launches no model, browser, or live product process.
use super::*;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

async fn web_body(api: &WorkspaceApi, method: &str, path: &str, body: Option<Value>) -> String {
    let response = build_router(api.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(
                    "authorization",
                    format!("Bearer api-token-{TEST_WORKSPACE_ID}"),
                )
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{method} {path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn web_pages(
    api: &WorkspaceApi,
    path: &str,
    cursor_field: &str,
    query_field: &str,
) -> Vec<String> {
    let mut pages = Vec::new();
    let mut next = None::<String>;
    loop {
        assert!(pages.len() < 10, "pagination failed to terminate");
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("limit", "1");
        if let Some(cursor) = next.as_deref() {
            query.append_pair(query_field, cursor);
        }
        let separator = if path.ends_with('?') { "" } else { "&" };
        let raw = web_body(
            api,
            "GET",
            &format!("{path}{separator}{}", query.finish()),
            None,
        )
        .await;
        let value: Value = serde_json::from_str(&raw).unwrap();
        next = value[cursor_field].as_str().map(str::to_owned);
        pages.push(raw);
        if next.is_none() {
            break;
        }
    }
    pages
}

#[tokio::test]
#[ignore = "requires Deno on PATH; deno task test:ticket:real-api selects this bridge explicitly"]
async fn ticket_web_roundtrip_preserves_retained_result_case_history_and_current_answer() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, worker, identity) = delivery_recording_fixture(dir.path()).await;
    let ticket = delivery_ticket(&api, "Retain completed research across reissue");
    let assigned = delivery_assign(&api, &ticket.id, "web-initial").await;
    let old_id = assigned["assignment"]["assignment_id"].as_str().unwrap();
    let initial = delivery_tracking(&api, &ticket.id, old_id);
    delivery_self_report(&api, &identity, &worker, &initial, "done", "web-done").await;
    let done = delivery_tracking(&api, &ticket.id, old_id);
    let base = format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{}", ticket.id);
    let detail_path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}", ticket.id);
    let done_detail = web_body(&api, "GET", &detail_path, None).await;
    let mut case_details = Vec::new();
    for (key, action) in [
        (
            "web-close",
            json!({"action":"close", "reason":"not_planned"}),
        ),
        ("web-reopen", json!({"action":"reopen"})),
    ] {
        let observed = api.authority.ticket(&ticket.id).unwrap();
        web_body(
            &api,
            "POST",
            &format!("{base}/case"),
            Some(json!({
                "expected_content_digest":observed.content_digest,
                "expected_state":observed.state,
                "expected_closure_operation_key":observed.closure_operation_key,
                "operation_key":key,"action":action,"explanation":"Independent human case decision"
            })),
        )
        .await;
        case_details.push(web_body(&api, "GET", &detail_path, None).await);
        assert_eq!(delivery_tracking(&api, &ticket.id, old_id), done);
    }
    let reassigned = delivery_assign(&api, &ticket.id, "web-reissue").await;
    let current_id = reassigned["assignment"]["assignment_id"].as_str().unwrap();
    assert_ne!(old_id, current_id);
    let current = delivery_tracking(&api, &ticket.id, current_id);
    let reissued_detail = web_body(&api, "GET", &detail_path, None).await;
    let retained = web_body(
        &api,
        "GET",
        &format!("{base}/assignment-tracking?assignment_id={old_id}"),
        None,
    )
    .await;
    let assignments = web_pages(
        &api,
        &format!("{base}/assignment-tracking/assignments?"),
        "next_assignment_id",
        "after_assignment_id",
    )
    .await;
    let history = web_pages(
        &api,
        &format!("{base}/assignment-tracking/history?assignment_id={old_id}"),
        "next_event_id",
        "after_event_id",
    )
    .await;
    // Use the actual public read as the Answer input, not the store expectation.
    let read = web_body(&api, "GET", &format!("{base}/assignment-tracking"), None).await;
    let rows: Value = serde_json::from_str(&read).unwrap();
    let answer_body = json!({"assignment_id":rows[0]["assignment_id"], "expected_last_event_id":rows[0]["last_event_id"], "operation_key":"web-current-answer", "body":"Continue the new request"});
    let receipt = web_body(
        &api,
        "POST",
        &format!("{base}/assignment-answer"),
        Some(answer_body.clone()),
    )
    .await;
    assert_eq!(
        web_body(
            &api,
            "POST",
            &format!("{base}/assignment-answer"),
            Some(answer_body)
        )
        .await,
        receipt
    );
    let after_answer = web_body(&api, "GET", &detail_path, None).await;
    assert_eq!(delivery_tracking(&api, &ticket.id, current_id), current);
    let wire = json!({
        "expected":{"workspace_id":TEST_WORKSPACE_ID,"ticket_id":ticket.id,
            "old_assignment_id":old_id,"new_assignment_id":current_id,
            "done_event_id":done.last_event_id,"new_event_id":current.last_event_id,
            "old_result":"Returned options without closing"},
        "done_detail":done_detail,"closed_detail":case_details[0],"reopened_detail":case_details[1],
        "reissued_detail":reissued_detail,"retained_old":retained,"assignment_pages":assignments,
        "old_history_pages":history,"answer_receipt":receipt,"answered_detail":after_answer
    });
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = tokio::process::Command::new("deno")
        .current_dir(root.join("web/workspace"))
        .args([
            "run",
            "--cached-only",
            "--config",
            "test/ticket-real-api/deno.json",
            "test/ticket-real-api/roundtrip.ts",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("explicit Ticket bridge requires Deno on PATH");
    // Include stdin backpressure inside the watchdog. Dropping the future kills
    // the child on timeout; wait_with_output reaps it on normal completion.
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), async move {
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(&serde_json::to_vec(&wire).unwrap())
            .await
            .unwrap();
        drop(stdin);
        child.wait_with_output().await.unwrap()
    })
    .await
    .expect("Ticket Web bridge exceeded 30 seconds");
    assert!(
        output.status.success(),
        "Ticket Web bridge failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("real Ticket bytes → production parser/operation/summary lifecycle passed")
    );
    execution.take_inputs();
    delivery_stop(&api, &worker);
}
