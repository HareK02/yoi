//! Real user-authenticated production router tests, sharing the Memory/Journal provenance fixture.
//! There is deliberately no Runtime adapter: reads must use saved state, not a source Worker.
use super::*;
use sha2::{Digest, Sha256};

fn subject(f: &Fixture) -> String {
    f.store
        .read_journal(&f.host, &f.journal_id)
        .unwrap()
        .subject_id
}
fn browser_path(subject: &str) -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/journal-browser")
}
fn browser_request(subject: &str, token: Option<&str>, body: serde_json::Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(browser_path(subject))
        .header(CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    request
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}
async fn browser_call(f: &Fixture, token: &str, body: serde_json::Value) -> serde_json::Value {
    let (status, bytes) = response(
        build_router(f.api.clone()),
        browser_request(&subject(f), Some(token), body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(bytes.len() <= MAX_JOURNAL_RESPONSE_BYTES);
    serde_json::from_slice(&bytes).unwrap()
}
fn detail(f: &Fixture) -> serde_json::Value {
    json!({"action":"entry","journal_id":f.journal_id,"entry_id":f.entry_id})
}
fn text(f: &Fixture, field: &str) -> serde_json::Value {
    json!({"action":"text","journal_id":f.journal_id,
        "entry_id": if field == "current_position" { None } else { Some(&f.entry_id) },
        "field":field,"expected_token":null,"byte_offset":0,"max_bytes":8192})
}

/// Adds another real saved Session/Journal, optionally correcting an entry in the first Session.
fn later_journal(f: &Fixture, correcting: Vec<String>) -> (String, String) {
    let session = Uuid::new_v4().to_string();
    let attr = f
        .store
        .record_session_attribution(
            crate::subjektiv::SubjectSessionAttribution::new(
                subject(f),
                RUNTIME,
                &f.source_worker.worker_id,
                &session,
            )
            .unwrap(),
        )
        .unwrap();
    let writer = f
        .store
        .claim_journal_writer(&subject(f), RUNTIME, &f.source_worker.worker_id)
        .unwrap();
    let host = f.store.bind_journal_session(&writer, attr).unwrap();
    let content = "A later Session revisits the constraint.";
    let range = JournalSourceRange {
        source: JournalSource {
            session_id: session,
            pointer: JournalSourcePointer {
                segment_id: Uuid::new_v4(),
                entry_ref: "E0".into(),
            },
            order: 0,
            content_sha256: Sha256::digest(content.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            total_bytes: content.len() as u64,
        },
        bytes: JournalByteRange {
            start: 0,
            end: content.len() as u64,
        },
    };
    let id = f
        .store
        .accept_journal_sources(&host, "Later Session", &[range.clone()])
        .unwrap();
    let binding = subjektiv::api::JobAttemptBinding {
        job_id: "later-fixture".into(),
        attempt_id: "one".into(),
    };
    let generation = f
        .store
        .prepare_journal_generation(&host, &id, binding.clone(), vec![range.clone()])
        .unwrap();
    let job = JournalJobContext::validated(&f.store, &host, &generation.id, binding).unwrap();
    subjektiv::journal::service::execute_bounded(
        &f.store,
        &job,
        JournalJobRequest::ReadText {
            read: JournalTextRead {
                target: JournalTextTarget::Source {
                    pointer: range.source.pointer.clone(),
                },
                offset: 0,
                limit: 8192,
                content_token: None,
            },
        },
        |_, _, _| {
            Ok(subjektiv::journal::recording::JournalMaterial {
                range: range.clone(),
                text: content.into(),
            })
        },
    )
    .unwrap();
    let entry = f
        .store
        .execute_journal(
            &job,
            JournalOperation {
                request_id: "later-entry".into(),
                expected_state: generation.initial_journal.state_token,
                action: JournalAction::Create {
                    title: "Later qualification".into(),
                    body: content.into(),
                    ongoing: String::new(),
                    state: JournalEntryState::Fixed,
                    source_end: Some(range.source.pointer.clone()),
                    correction_refs: correcting,
                },
                covered_ranges: vec![range],
            },
        )
        .unwrap()
        .entry_id
        .unwrap();
    (id, entry)
}

#[tokio::test]
async fn journal_browser_user_auth_subject_boundary_and_read_only_actions() {
    let f = Fixture::new().await;
    let token = seed_test_workspace_owner_token(f.api.store.as_ref(), "journal-browser-user").await;
    let app = build_router(f.api.clone());
    assert_eq!(
        response(app.clone(), browser_request(&subject(&f), None, detail(&f)))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let signed = runtime_source_request(
        &f.identity,
        Some(&f.worker.worker_id),
        "POST",
        &browser_path(&subject(&f)),
        serde_json::to_vec(&detail(&f)).unwrap(),
    );
    assert_eq!(response(app.clone(), signed).await.0, StatusCode::FORBIDDEN);
    let other = f
        .store
        .create_subject(crate::subjektiv::SubjectRole::new("other-browser").unwrap())
        .unwrap();
    for body in [
        detail(&f),
        text(&f, "body"),
        json!({"action":"entries","journal_id":f.journal_id,"query":null,"after":null,"limit":10}),
        json!({"action":"memory_sources","selector":f.selector,"source_offset":0,"limit":10}),
        json!({"action":"related_memory","journal_id":f.journal_id,"entry_id":f.entry_id,"after":null,"limit":10}),
    ] {
        let (status, bytes) =
            response(app.clone(), browser_request(&other.id, Some(&token), body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!String::from_utf8_lossy(&bytes).contains(JOURNAL_BODY));
    }
    let empty = response(
        app.clone(),
        browser_request(
            &other.id,
            Some(&token),
            json!({"action":"sessions","after":null,"limit":10}),
        ),
    )
    .await;
    assert_eq!(empty.0, StatusCode::OK);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&empty.1).unwrap()["value"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let unknown_action = browser_request(
        &subject(&f),
        Some(&token),
        json!({"action":"reconcile","selector":f.selector}),
    );
    assert_eq!(
        response(app, unknown_action).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let entry = browser_call(&f, &token, detail(&f)).await;
    assert_eq!(entry["value"]["entry"]["state"], "open");
    assert_eq!(entry["value"]["entry"]["title"], "Choosing the approach");
    assert_eq!(entry["value"]["original_source"]["state"], "unavailable");
}

#[tokio::test]
async fn journal_browser_multiple_sessions_recent_search_and_filter_bound_paging() {
    let f = Fixture::new().await;
    f.fix_initial();
    let (later, later_entry) = later_journal(&f, vec![]);
    let token = seed_test_workspace_owner_token(f.api.store.as_ref(), "browser-pages").await;
    let first = browser_call(
        &f,
        &token,
        json!({"action":"sessions","after":null,"limit":1}),
    )
    .await;
    assert_eq!(first["value"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["value"]["items"][0]["unreflected_sources"], true);
    let second = browser_call(
        &f,
        &token,
        json!({"action":"sessions","after":first["value"]["next"],"limit":1}),
    )
    .await;
    assert_eq!(second["value"]["items"].as_array().unwrap().len(), 1);
    assert_ne!(
        first["value"]["items"][0]["journal"]["id"],
        second["value"]["items"][0]["journal"]["id"]
    );
    assert!(second["value"]["next"].is_null());
    let first = browser_call(
        &f,
        &token,
        json!({"action":"entries","journal_id":null,"query":null,"after":null,"limit":1}),
    )
    .await;
    assert_eq!(first["value"]["items"][0]["id"], later_entry);
    let second = browser_call(&f, &token, json!({"action":"entries","journal_id":null,"query":null,"after":first["value"]["next"],"limit":1})).await;
    assert_eq!(second["value"]["items"][0]["id"], f.entry_id);
    assert!(second["value"]["next"].is_null());
    let filtered = browser_call(
        &f,
        &token,
        json!({"action":"entries","journal_id":later,"query":"REVISITS","after":null,"limit":32}),
    )
    .await;
    assert_eq!(filtered["value"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["value"]["items"][0]["id"], later_entry);
    for query in ["no such event", "%"] {
        let empty = browser_call(
            &f,
            &token,
            json!({"action":"entries","journal_id":null,"query":query,"after":null,"limit":32}),
        )
        .await;
        assert!(empty["value"]["items"].as_array().unwrap().is_empty());
    }
    let changed_filter = browser_request(
        &subject(&f),
        Some(&token),
        json!({"action":"entries","journal_id":later,
        "query":null,"after":first["value"]["next"],"limit":1}),
    );
    assert_eq!(
        response(build_router(f.api.clone()), changed_filter)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let other = f
        .store
        .create_subject(crate::subjektiv::SubjectRole::new("cursor-other").unwrap())
        .unwrap();
    let cross_subject = browser_request(
        &other.id,
        Some(&token),
        json!({"action":"entries","journal_id":null,
        "query":null,"after":first["value"]["next"],"limit":1}),
    );
    assert_eq!(
        response(build_router(f.api.clone()), cross_subject).await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn journal_browser_text_bounds_tokens_position_and_ongoing_are_independent() {
    let f = Fixture::new().await;
    let token = seed_test_workspace_owner_token(f.api.store.as_ref(), "browser-text").await;
    assert_eq!(
        browser_call(&f, &token, text(&f, "ongoing")).await["value"]["text"],
        "Validation pending"
    );
    assert_eq!(
        browser_call(&f, &token, text(&f, "current_position")).await["value"]["text"],
        ""
    );
    let mut request = text(&f, "body");
    request["max_bytes"] = json!(8);
    let first = browser_call(&f, &token, request.clone()).await;
    assert_eq!(first["value"]["text"], &JOURNAL_BODY[..8]);
    request["byte_offset"] = first["value"]["next_byte"].clone();
    let (status, _) = response(
        build_router(f.api.clone()),
        browser_request(&subject(&f), Some(&token), request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    request["expected_token"] = first["value"]["content_token"].clone();
    let next = browser_call(&f, &token, request.clone()).await;
    assert_eq!(next["value"]["text"], &JOURNAL_BODY[8..16]);
    f.fix_initial(); // changes entry token, never tears the old continuation
    assert_eq!(
        response(
            build_router(f.api.clone()),
            browser_request(&subject(&f), Some(&token), request)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    for body in [
        json!({"action":"sessions","after":null,"limit":0}),
        json!({"action":"entries","journal_id":null,"query":null,"after":null,"limit":33}),
        json!({"action":"entries","journal_id":null,"query":"x".repeat(257),"after":null,"limit":10}),
        {
            let mut body = text(&f, "body");
            body["max_bytes"] = json!(16385);
            body
        },
    ] {
        assert_eq!(
            response(
                build_router(f.api.clone()),
                browser_request(&subject(&f), Some(&token), body)
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn journal_browser_saved_bidirectional_links_titles_and_cross_session_corrections_survive_deletion()
 {
    let f = Fixture::new().await;
    let token = seed_test_workspace_owner_token(f.api.store.as_ref(), "browser-links").await;
    let sources =
        json!({"action":"memory_sources","selector":f.selector,"source_offset":0,"limit":1});
    let unconnected = browser_call(&f, &token, sources.clone()).await;
    assert_eq!(
        unconnected["value"]["sources"][0]["link"]["connection"]["state"],
        "unconnected"
    );
    assert_eq!(
        unconnected["value"]["sources"][0]["link"]["connection"]["reason"],
        "not_reconciled"
    );
    f.fix_initial();
    f.call(MemoryJournalRequest::Reconcile {
        selector: f.selector.clone(),
    })
    .await;
    let (later, correction) = later_journal(&f, vec![f.entry_id.clone()]);
    let linked = browser_call(&f, &token, sources.clone()).await;
    assert_eq!(
        linked["value"]["sources"][0]["entry"]["title"],
        "Choosing the approach"
    );
    assert!(linked["value"]["sources"][0]["entry"]["created_at"].is_string());
    assert_eq!(
        linked["value"]["sources"][0]["link"]["connection"]["entry_id"],
        f.entry_id
    );
    let corrections = browser_call(
        &f,
        &token,
        json!({"action":"corrections","journal_id":f.journal_id,
        "entry_id":f.entry_id,"after":null,"limit":1}),
    )
    .await;
    assert_eq!(corrections["value"]["items"][0]["id"], correction);
    assert_eq!(corrections["value"]["items"][0]["journal_id"], later);
    let corrected = browser_call(
        &f,
        &token,
        json!({"action":"entry","journal_id":later,"entry_id":correction}),
    )
    .await;
    assert_eq!(
        corrected["value"]["correction_targets"][0]["id"],
        f.entry_id
    );
    assert_eq!(
        corrected["value"]["correction_targets"][0]["journal_id"],
        f.journal_id
    );
    let related_request = json!({"action":"related_memory","journal_id":f.journal_id,"entry_id":f.entry_id,"after":null,"limit":1});
    let related = browser_call(&f, &token, related_request.clone()).await;
    assert_eq!(
        related["value"]["items"][0]["claim"],
        "Check the constraint before choosing"
    );
    assert_eq!(
        related["value"]["items"][0]["related"]["selector"]["change_id"],
        f.selector.change_id
    );
    assert_eq!(
        related["value"]["items"][0]["related"]["reconsideration"],
        true
    );
    fs::remove_dir_all(&f.source_root).unwrap();
    f.api
        .store
        .delete_worker_registry(TEST_WORKSPACE_ID, &f.source_worker)
        .unwrap();
    let deleted = browser_call(&f, &token, detail(&f)).await;
    assert_eq!(deleted["value"]["original_source"]["state"], "missing");
    assert!(deleted["value"]["original_source"]["history_href"].is_null());
    assert_eq!(
        browser_call(&f, &token, text(&f, "body")).await["value"]["text"],
        JOURNAL_BODY
    );
    let linked = browser_call(&f, &token, sources).await;
    assert_eq!(
        linked["value"]["sources"][0]["original_source"]["state"],
        "missing"
    );
    assert_eq!(linked["value"]["sources"][0]["entry"]["id"], f.entry_id);
    assert_eq!(
        browser_call(&f, &token, related_request).await["value"]["items"][0]["claim"],
        "Check the constraint before choosing"
    );
}

#[tokio::test]
async fn journal_browser_history_navigation_requires_exact_session_mapping_not_registry_presence() {
    let f = Fixture::new().await;
    let token = seed_test_workspace_owner_token(f.api.store.as_ref(), "browser-original").await;
    let mut registry = f
        .api
        .store
        .get_worker_registry(TEST_WORKSPACE_ID, &f.source_worker)
        .unwrap()
        .unwrap();
    let initial = browser_call(&f, &token, detail(&f)).await;
    assert!(initial["value"]["original_source"]["history_href"].is_null());
    registry.session_ref = Some(f.ranges[0].source.session_id.clone());
    f.api.store.upsert_worker_registry(&registry).unwrap();
    let exact = browser_call(&f, &token, detail(&f)).await;
    let resource_key = f
        .api
        .store
        .worker_resource_key(TEST_WORKSPACE_ID, &f.source_worker)
        .unwrap()
        .unwrap();
    assert_eq!(
        exact["value"]["original_source"]["history_href"],
        browser_worker_console_href(TEST_WORKSPACE_ID, &resource_key)
    );
    assert!(resource_key.starts_with("W-"));
    // Metadata does not attest original bytes. A later Session must not be linked as the old one.
    assert_eq!(exact["value"]["original_source"]["state"], "unavailable");
    registry.session_ref = Some("different-session".into());
    f.api.store.upsert_worker_registry(&registry).unwrap();
    assert!(
        browser_call(&f, &token, detail(&f)).await["value"]["original_source"]["history_href"]
            .is_null()
    );
}

#[tokio::test]
async fn journal_browser_many_saved_links_page_without_reconciliation_and_use_immutable_claims() {
    let f = Fixture::new().await;
    f.fix_initial();
    let correction = f.add_correction();
    f.call(MemoryJournalRequest::Reconcile {
        selector: f.selector.clone(),
    })
    .await;
    let token = seed_test_workspace_owner_token(f.api.store.as_ref(), "browser-many-links").await;
    let mut candidate = f
        .store
        .staging_candidate(&subject(&f), &f.selector.candidate_id)
        .unwrap()
        .unwrap();
    candidate.id = "second-browser-candidate".into();
    candidate.claim = "Another immutable lesson from both events".into();
    let mut source = candidate.source_refs[0].clone();
    source.evidence_id = Some(f.ranges[1].source.pointer.entry_ref.clone());
    source.entry_range = Some([1, 1]);
    let mut evidence = candidate.evidence[0].clone();
    evidence.id = source.evidence_id.clone().unwrap();
    evidence.entry_range = Some([1, 1]);
    candidate.source_refs.push(source);
    candidate.evidence.push(evidence);
    let (candidate, _) = f
        .store
        .stage_candidate_with_attribution(
            candidate,
            f.store
                .session_attribution(&f.ranges[0].source.session_id)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    let (memory, _) = f
        .store
        .apply_candidate(
            &subject(&f),
            &candidate.id,
            crate::subjektiv::MemoryChangeTarget::Create,
            crate::subjektiv::MemoryDraft::active(
                candidate.kind.clone(),
                &candidate.claim,
                "Saved Memory body",
                &candidate.why_useful,
                "Second adoption",
            ),
            "Reviewed",
        )
        .unwrap();
    let selector = MemoryJournalSelector {
        memory_id: memory.id.clone(),
        change_id: memory.change_id.clone(),
        candidate_id: candidate.id.clone(),
    };
    let first = browser_call(
        &f,
        &token,
        json!({"action":"memory_sources","selector":selector,"source_offset":0,"limit":1}),
    )
    .await;
    assert_eq!(first["value"]["total_sources"], 2);
    assert_eq!(first["value"]["next_source_index"], 1);
    assert_eq!(
        first["value"]["sources"][0]["link"]["connection"]["reason"],
        "not_reconciled"
    );
    // Reading a browser page never reconciles. Only the existing domain fixture explicitly does so.
    f.call(MemoryJournalRequest::Reconcile {
        selector: selector.clone(),
    })
    .await;
    let first = browser_call(
        &f,
        &token,
        json!({"action":"memory_sources","selector":selector,"source_offset":0,"limit":1}),
    )
    .await;
    let next = browser_call(&f,&token,json!({"action":"memory_sources","selector":selector,"source_offset":first["value"]["next_source_index"],"limit":1})).await;
    assert_eq!(first["value"]["sources"][0]["entry"]["id"], f.entry_id);
    assert_eq!(next["value"]["sources"][0]["entry"]["id"], correction);
    assert!(next["value"]["next_source_index"].is_null());
    f.store
        .revise_memory(
            &subject(&f),
            &memory.id,
            memory.change_id,
            crate::subjektiv::MemoryDraft::active(
                candidate.kind,
                "Changed current claim",
                "New body",
                "Updated lesson",
                "Later qualification",
            ),
        )
        .unwrap();
    let first = browser_call(&f,&token,json!({"action":"related_memory","journal_id":f.journal_id,"entry_id":f.entry_id,"after":null,"limit":1})).await;
    let next = browser_call(&f,&token,json!({"action":"related_memory","journal_id":f.journal_id,"entry_id":f.entry_id,"after":first["value"]["next"],"limit":1})).await;
    assert!(first["value"]["next"].is_object());
    assert!(next["value"]["next"].is_null());
    let claims = [
        first["value"]["items"][0]["claim"].as_str().unwrap(),
        next["value"]["items"][0]["claim"].as_str().unwrap(),
    ];
    assert!(claims.contains(&"Another immutable lesson from both events"));
    assert!(claims.contains(&"Check the constraint before choosing"));
    assert!(!claims.contains(&"Changed current claim"));
}

#[tokio::test]
async fn journal_browser_read_post_has_memory_get_user_authority_but_no_mutation_authority() {
    let f = Fixture::new().await;
    let token = seed_test_api_token(f.api.store.as_ref(), "browser-reader-not-owner");
    // Ordinary authenticated read authority is the same as Memory GET, even though browser
    // selectors use POST. This exception is exact-route only; no mutations gain authority.
    assert_eq!(
        browser_call(&f, &token, detail(&f)).await["value"]["entry"]["id"],
        f.entry_id
    );
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects"))
        .header(CONTENT_TYPE, "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(r#"{"role":"not-allowed"}"#))
        .unwrap();
    assert_eq!(
        response(build_router(f.api.clone()), request).await.0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn journal_browser_saved_missing_bytes_do_not_claim_original_conversation_deleted() {
    let f = Fixture::new().await;
    f.fix_initial();
    f.call(MemoryJournalRequest::Reconcile {
        selector: f.selector.clone(),
    })
    .await;
    let token = seed_test_workspace_owner_token(f.api.store.as_ref(), "browser-missing-byte").await;
    let mut registry = f
        .api
        .store
        .get_worker_registry(TEST_WORKSPACE_ID, &f.source_worker)
        .unwrap()
        .unwrap();
    registry.session_ref = Some(f.ranges[0].source.session_id.clone());
    f.api.store.upsert_worker_registry(&registry).unwrap();
    let sources =
        json!({"action":"memory_sources","selector":f.selector,"source_offset":0,"limit":1});
    let present = browser_call(&f, &token, sources.clone()).await;
    assert!(present["value"]["sources"][0]["original_source"]["history_href"].is_string());

    // The Host attests only a missing byte of the referenced source. Original Session
    // metadata and material still exist; neither this attestation nor its saved link
    // projection certifies deletion of the conversation as a whole.
    let mut missing_byte = f.ranges[0].clone();
    missing_byte.bytes.end = missing_byte.bytes.start + 1;
    f.store
        .mark_journal_sources_unavailable(&f.host, &f.journal_id, &[missing_byte])
        .unwrap();
    assert!(f.source_root.exists());
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.source_worker)
            .unwrap()
            .is_some()
    );
    let partial = browser_call(&f, &token, sources.clone()).await;
    let source = &partial["value"]["sources"][0];
    assert_eq!(source["link"]["original_source"], "missing");
    assert_eq!(source["original_source"]["state"], "unavailable");
    assert!(source["original_source"]["history_href"].is_null());
    assert_eq!(source["entry"]["id"], f.entry_id);
    assert_eq!(
        browser_call(&f, &token, text(&f, "body")).await["value"]["text"],
        JOURNAL_BODY
    );

    // Only independently confirmed registry/archive absence changes the conversation
    // status to missing. The same saved Journal and immutable link remain readable.
    fs::remove_dir_all(&f.source_root).unwrap();
    f.api
        .store
        .delete_worker_registry(TEST_WORKSPACE_ID, &f.source_worker)
        .unwrap();
    let deleted = browser_call(&f, &token, sources).await;
    assert_eq!(
        deleted["value"]["sources"][0]["original_source"]["state"],
        "missing"
    );
    assert!(deleted["value"]["sources"][0]["original_source"]["history_href"].is_null());
    assert_eq!(deleted["value"]["sources"][0]["entry"]["id"], f.entry_id);
}

/// Emits actual authenticated router/store responses for the production Web contract runner.
/// The paired `test:journal:backend-contract` task validates all action payloads with the real
/// Web parser; this Rust test remains usable without a JavaScript runtime or product process.
#[tokio::test]
async fn journal_browser_web_contract_serializes_saved_router_responses() {
    let f = Fixture::new().await;
    f.fix_initial();
    f.call(MemoryJournalRequest::Reconcile {
        selector: f.selector.clone(),
    })
    .await;
    let (later, correction) = later_journal(&f, vec![f.entry_id.clone()]);
    let token = seed_test_workspace_owner_token(f.api.store.as_ref(), "browser-web-contract").await;
    let mut registry = f
        .api
        .store
        .get_worker_registry(TEST_WORKSPACE_ID, &f.source_worker)
        .unwrap()
        .unwrap();
    registry.session_ref = Some(f.ranges[0].source.session_id.clone());
    f.api.store.upsert_worker_registry(&registry).unwrap();
    let sources =
        json!({"action":"memory_sources","selector":f.selector,"source_offset":0,"limit":3});
    let requests = [
        json!({"action":"sessions","after":null,"limit":20}),
        json!({"action":"entries","journal_id":null,"query":null,"after":null,"limit":20}),
        detail(&f),
        text(&f, "body"),
        json!({"action":"corrections","journal_id":f.journal_id,"entry_id":f.entry_id,"after":null,"limit":10}),
        sources.clone(),
        json!({"action":"related_memory","journal_id":f.journal_id,"entry_id":f.entry_id,"after":null,"limit":3}),
        json!({"action":"entry","journal_id":later,"entry_id":correction}),
    ];
    for request in requests {
        let response = browser_call(&f, &token, request.clone()).await;
        println!(
            "JOURNAL_BROWSER_CONTRACT {}",
            json!({"workspace_id":TEST_WORKSPACE_ID,"subject_id":subject(&f),"request":request,"response":response})
        );
    }
    fs::remove_dir_all(&f.source_root).unwrap();
    f.api
        .store
        .delete_worker_registry(TEST_WORKSPACE_ID, &f.source_worker)
        .unwrap();
    for request in [detail(&f), sources] {
        let response = browser_call(&f, &token, request.clone()).await;
        println!(
            "JOURNAL_BROWSER_CONTRACT {}",
            json!({"workspace_id":TEST_WORKSPACE_ID,"subject_id":subject(&f),"request":request,"response":response})
        );
    }
}

#[tokio::test]
async fn journal_browser_cookie_read_post_retains_origin_check_for_non_owner_reader() {
    let f = Fixture::new().await;
    seed_test_api_token(f.api.store.as_ref(), "journal-cookie-reader");
    f.api
        .store
        .create_browser_session(&BrowserSessionRecord {
            token_hash: crate::auth::token_hash("journal-cookie-session"),
            session_id: "journal-cookie-session-id".into(),
            user_id: "user-journal-cookie-reader".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            revoked_at: None,
        })
        .unwrap();
    let AuthConfig::Passkey {
        origin,
        cookie_name,
        ..
    } = &f.api.config.auth;
    for (presented, expected) in [
        (None, StatusCode::FORBIDDEN),
        (Some("https://foreign.example"), StatusCode::FORBIDDEN),
        (Some(origin.as_str()), StatusCode::OK),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri(browser_path(&subject(&f)))
            .header(CONTENT_TYPE, "application/json")
            .header(
                axum::http::header::COOKIE,
                format!("{cookie_name}=journal-cookie-session"),
            );
        if let Some(origin) = presented {
            request = request.header(axum::http::header::ORIGIN, origin);
        }
        let (status, bytes) = response(
            build_router(f.api.clone()),
            request
                .body(Body::from(serde_json::to_vec(&detail(&f)).unwrap()))
                .unwrap(),
        )
        .await;
        assert_eq!(status, expected);
        if status == StatusCode::OK {
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["value"]["entry"]["id"], f.entry_id);
        }
    }
}
