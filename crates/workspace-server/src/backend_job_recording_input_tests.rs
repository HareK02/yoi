//! Real SQLite extension tests use the production store and captured DTO contract.
//! Synthetic child outcomes test store fencing, not Runtime termination or Journal reflection.
use super::*;
use session_store::capture::{CaptureSource, CaptureUsePolicy};
use session_store::public_material::{
    PublicMaterialDescriptor, PublicMaterialIdentity, PublicMaterialOrigin, PublicRunBoundary,
};

const SEGMENT: &str = "019a4ea8-0000-7000-8000-000000000001";
const NOW: &str = "2026-11-04T00:00:00Z";

fn store() -> SqliteWorkspaceStore {
    let store = SqliteWorkspaceStore::in_memory_fixture().unwrap();
    store.with_conn(|conn| {
        let installed:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='backend_job_recording_inputs')",[],|r|r.get(0))?;
        if !installed { conn.execute_batch(SCHEMA_SQL)?; }
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('w','owner','Workspace','active','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('other','owner','Other','active','1','1');")?;
        Ok(())
    }).unwrap();
    store
}

fn scope() -> BackendJobRecordingScope {
    BackendJobRecordingScope {
        subject_id: "subject".into(),
        session_id: "session".into(),
        binding_id: "lifetime".into(),
        worker: RuntimeWorkerRef::new("removed-runtime", "removed-worker"),
        database: DatabaseIdentity {
            scope: "scope".into(),
            feature: "subjektiv".into(),
            physical: "database".into(),
        },
        start: RecordingPosition::default(),
        end: RecordingPosition {
            canonical_order: 1,
            byte_offset: 0,
        },
    }
}

fn capture() -> CaptureDto {
    let boundary = PublicRunBoundary::Yielded;
    let text = serde_json::json!({"kind":"run_boundary","boundary":boundary}).to_string();
    CaptureDto {
        version: 1,
        session_id: "session".into(),
        segment_id: SEGMENT.into(),
        view_entry_count: 1,
        entries: vec![CaptureEntryDto {
            entry_id: "1".into(),
            order: 0,
            source: CaptureSource {
                segment_id: SEGMENT.into(),
                entry_index: 0,
                provenance: protocol::SessionEntryProvenance::BackendInstruction,
                history_origin: None,
                derived_from: Vec::new(),
            },
            policy: CaptureUsePolicy {
                recording_material: true,
                memory_evidence: false,
                observation_readable: false,
            },
            body: CaptureBody::CanonicalPublic {
                descriptor: PublicMaterialDescriptor {
                    identity: PublicMaterialIdentity {
                        segment_id: SEGMENT.parse().unwrap(),
                        entry_ref: "E1".into(),
                    },
                    canonical_order: 0,
                    origin: PublicMaterialOrigin::HostRunBoundary,
                    sha256: digest(text.as_bytes()),
                    total_bytes: text.len() as u64,
                },
                byte_offset: 0,
                text,
                boundary: Some(boundary),
            },
        }],
    }
}

fn split() -> Vec<CaptureDto> {
    let whole = capture();
    let CaptureBody::CanonicalPublic { text, .. } = &whole.entries[0].body else {
        unreachable!()
    };
    let cut = text.len() / 2;
    let mut first = whole.clone();
    let mut second = whole.clone();
    if let CaptureBody::CanonicalPublic { text: fragment, .. } = &mut first.entries[0].body {
        *fragment = text[..cut].into();
    }
    if let CaptureBody::CanonicalPublic {
        text: fragment,
        byte_offset,
        ..
    } = &mut second.entries[0].body
    {
        *fragment = text[cut..].into();
        *byte_offset = cut as u64;
    }
    vec![first, second]
}

fn request(id: &str, input: Value) -> BackendJobRequest {
    serde_json::from_value(serde_json::json!({"job_id":id,"purpose":"finite_recording","input_ref":"saved:recording","input":input,"instruction":"Execute the saved finite recording","profile":"builtin:backend-job"})).unwrap()
}
fn reserve(store: &SqliteWorkspaceStore, id: &str) -> BackendJobRecordingReservation {
    let scope = scope();
    let chunks = split();
    let request = request(id, scope.request_input(&chunks).unwrap());
    store
        .reserve_backend_job_recording_input("w", &request, &scope, &chunks, NOW)
        .unwrap()
}
fn child_request(id: &str) -> BackendJobRequest {
    let mut request = request(id, serde_json::json!({"saved":"bounded prefix"}));
    request.grants=serde_json::from_value(serde_json::json!({"subjektiv_journal":{
        "binding_id":"lifetime","subject_id":"subject","session_id":"session","journal_id":"journal",
        "selected_ranges":[{"source":{"session_id":"session","pointer":{"segment_id":SEGMENT,"entry_ref":"E1"},"order":0,"content_sha256":"2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824","total_bytes":5},"bytes":{"start":0,"end":5}}]
    }})).unwrap();
    request.serialization_key = Some(crate::backend_job::journal_serialization_key("journal"));
    request
}
fn child(store: &SqliteWorkspaceStore, id: &str) -> BackendJobReservation {
    store
        .reserve_backend_job("w", &child_request(id), NOW)
        .unwrap()
}
fn settle_child(store: &SqliteWorkspaceStore, id: &str) {
    // No Runtime worker was allocated by these fixtures. The ordinary reserved
    // attempt failure path records allocation preclusion and releases its owner.
    let attempt = store
        .list_backend_job_attempts("w", id)
        .unwrap()
        .pop()
        .unwrap();
    store
        .finish_backend_job_attempt(
            "w",
            id,
            &attempt.attempt_id,
            BackendJobAttemptState::Failed,
            "fixture_rejected",
            "No Worker allocated",
            NOW,
        )
        .unwrap();
    assert_eq!(
        store.get_backend_job("w", id).unwrap().unwrap().state,
        BackendJobState::Failed
    );
}

#[test]
fn reservation_owns_complete_captured_bytes_without_a_source_worker_and_replays_exact_intent() {
    let store = store();
    let first = reserve(&store, "host");
    assert!(!first.replayed);
    assert_eq!(first.job.state, BackendJobState::Pending);
    assert_eq!(first.input.chunk_count, 2);
    assert_eq!(
        first.job.request.input,
        scope().request_input(&split()).unwrap()
    );
    assert!(
        store
            .list_backend_job_attempts("w", "host")
            .unwrap()
            .is_empty()
    );
    let again = reserve(&store, "host");
    assert!(again.replayed);
    assert_eq!(again.job, first.job);
    assert_eq!(again.input, first.input);
    first.job.load_executable_request().unwrap();
    assert_eq!(
        store
            .list_active_backend_job_recording_ids("w", None, 1)
            .unwrap(),
        ["host"]
    );
    assert!(
        store
            .list_active_backend_job_recording_ids("w", Some("host"), 1)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_active_backend_job_recording_ids("other", None, 10)
            .unwrap()
            .is_empty()
    );
    let mut changed = scope();
    changed.binding_id = "different-lifetime".into();
    let request = request("host", changed.request_input(&split()).unwrap());
    assert!(
        store
            .reserve_backend_job_recording_input("w", &request, &changed, &split(), NOW)
            .is_err()
    );
    assert_eq!(
        store.get_backend_job_recording_input("w", "host").unwrap(),
        Some(first.input)
    );
}

#[test]
fn chunk_pages_are_bounded_ordered_and_workspace_scoped() {
    let store = store();
    reserve(&store, "host");
    let first = store
        .get_backend_job_recording_chunks("w", "host", 0, 1)
        .unwrap();
    assert_eq!(
        first.chunks,
        [BackendJobRecordingChunk {
            index: 0,
            capture: split()[0].clone()
        }]
    );
    assert_eq!(first.next_index, Some(1));
    let second = store
        .get_backend_job_recording_chunks("w", "host", 1, 1)
        .unwrap();
    assert_eq!(second.chunks[0].capture, split()[1]);
    assert_eq!(second.next_index, None);
    assert!(
        store
            .get_backend_job_recording_chunks("w", "host", 2, 1)
            .unwrap()
            .chunks
            .is_empty()
    );
    for (start, limit) in [(0, 0), (0, MAX_RECORDING_CHUNK_PAGE + 1), (3, 1)] {
        assert!(
            store
                .get_backend_job_recording_chunks("w", "host", start, limit)
                .is_err()
        );
    }
    assert!(
        store
            .get_backend_job_recording_chunks("other", "host", 0, 1)
            .is_err()
    );
}

#[test]
fn capture_validation_rejects_gaps_overlap_descriptor_changes_wrong_session_and_missing_tail() {
    let scope = scope();
    let chunks = split();
    let mut gap = chunks.clone();
    if let CaptureBody::CanonicalPublic {
        byte_offset, text, ..
    } = &mut gap[1].entries[0].body
    {
        *byte_offset += 1;
        text.remove(0);
    }
    assert!(prepare(&scope, &gap).is_err());
    let mut overlap = chunks.clone();
    if let CaptureBody::CanonicalPublic { byte_offset, .. } = &mut overlap[1].entries[0].body {
        *byte_offset -= 1;
    }
    assert!(prepare(&scope, &overlap).is_err());
    let mut changed = chunks.clone();
    if let CaptureBody::CanonicalPublic { descriptor, .. } = &mut changed[1].entries[0].body {
        descriptor.sha256 = "0".repeat(64);
    }
    assert!(prepare(&scope, &changed).is_err());
    let mut wrong = chunks.clone();
    wrong[1].session_id = "another-session".into();
    assert!(prepare(&scope, &wrong).is_err());
    assert!(prepare(&scope, &chunks[..1]).is_err());
    let mut wrong_bytes = chunks.clone();
    if let CaptureBody::CanonicalPublic { text, .. } = &mut wrong_bytes[1].entries[0].body {
        text.replace_range(..1, "X");
    }
    assert!(prepare(&scope, &wrong_bytes).is_err());
    let mut changed_metadata = chunks.clone();
    changed_metadata[1].entries[0].source.entry_index = 1;
    assert!(prepare(&scope, &changed_metadata).is_err());
    let mut wrong_view = chunks.clone();
    wrong_view[1].segment_id = "other-segment".into();
    assert!(prepare(&scope, &wrong_view).is_err());
    let mut noncanonical = capture();
    noncanonical.entries[0].body = CaptureBody::PublicSystem {
        text: "public but not recording canonical".into(),
    };
    assert!(prepare(&scope, &[noncanonical]).is_err());
}

#[test]
fn empty_scope_is_explicit_and_scope_digest_binds_database_positions_and_all_captured_bytes() {
    let mut empty = scope();
    assert!(prepare(&empty, &[]).is_err());
    empty.end = empty.start;
    assert!(prepare(&empty, &[]).is_ok());
    let original = scope().recording_scope_digest(&split()).unwrap();
    let mut changed = scope();
    changed.database.physical = "replacement".into();
    assert_ne!(changed.recording_scope_digest(&split()).unwrap(), original);
    let mut changed = scope();
    changed.worker.worker_id = "replacement".into();
    assert_ne!(changed.recording_scope_digest(&split()).unwrap(), original);
}

#[test]
fn oversized_and_excessive_captures_are_rejected_before_any_job_is_reserved() {
    let store = store();
    let scope = scope();
    let mut huge = capture();
    if let CaptureBody::CanonicalPublic {
        text,
        descriptor,
        boundary,
        ..
    } = &mut huge.entries[0].body
    {
        *text = "x".repeat(MAX_RECORDING_CHUNK_BYTES);
        descriptor.total_bytes = text.len() as u64;
        descriptor.sha256 = digest(text.as_bytes());
        *boundary = Some(PublicRunBoundary::Yielded);
    }
    assert!(prepare(&scope, &[huge]).is_err());
    assert!(prepare(&scope, &vec![capture(); MAX_RECORDING_CHUNKS + 1]).is_err());
    assert!(
        store
            .reserve_backend_job_recording_input(
                "w",
                &request("bad", serde_json::json!({"recording_scope_digest":"wrong"})),
                &scope,
                &split(),
                NOW
            )
            .is_err()
    );
    assert!(store.get_backend_job("w", "bad").unwrap().is_none());
}

#[test]
fn chunk_insert_failure_rolls_back_parent_scope_and_chunks() {
    let store = store();
    store.with_conn(|conn| {
        conn.execute_batch("CREATE TEMP TRIGGER reject_second_recording_chunk BEFORE INSERT ON main.backend_job_recording_chunks WHEN NEW.chunk_index=1 BEGIN SELECT RAISE(ABORT,'injected chunk failure'); END;")?; Ok(())
    }).unwrap();
    let scope = scope();
    let chunks = split();
    let request = request("host", scope.request_input(&chunks).unwrap());
    assert!(
        store
            .reserve_backend_job_recording_input("w", &request, &scope, &chunks, NOW)
            .unwrap_err()
            .to_string()
            .contains("injected chunk failure")
    );
    assert!(store.get_backend_job("w", "host").unwrap().is_none());
    assert!(
        store
            .get_backend_job_recording_input("w", "host")
            .unwrap()
            .is_none()
    );
    store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER reject_second_recording_chunk")?;
            Ok(())
        })
        .unwrap();
    assert!(
        !store
            .reserve_backend_job_recording_input("w", &request, &scope, &chunks, NOW)
            .unwrap()
            .replayed
    );
}

#[test]
fn child_reservation_callback_and_binding_share_one_rollback_boundary() {
    let store = store();
    reserve(&store, "host");
    let child = child(&store, "child");
    let original = child.job.clone();
    let failed = store.reserve_backend_job_recording_child_with(
        "w", "host", Some("stale-child"), |conn| {
            conn.execute("UPDATE backend_jobs SET updated_at='callback-write' WHERE workspace_id='w' AND job_id='child'", [])?;
            Ok(child.clone())
        },
    );
    assert!(matches!(failed, Err(Error::RepositoryConflict(_))));
    assert_eq!(
        store.get_backend_job("w", "child").unwrap().unwrap(),
        original
    );
    assert_eq!(
        store
            .get_backend_job_recording_input("w", "host")
            .unwrap()
            .unwrap()
            .current_child_job_id,
        None
    );
    let (input, saved) = store
        .reserve_backend_job_recording_child_with("w", "host", None, |_| Ok(child.clone()))
        .unwrap();
    assert_eq!(input.current_child_job_id.as_deref(), Some("child"));
    assert_eq!(saved.job, original);
}

#[test]
fn database_identity_wire_format_is_strict_and_round_trips() {
    let original = scope();
    let mut wire = serde_json::to_value(&original).unwrap();
    assert_eq!(
        serde_json::from_value::<BackendJobRecordingScope>(wire.clone()).unwrap(),
        original
    );
    wire["database"]["unexpected"] = Value::Bool(true);
    assert!(serde_json::from_value::<BackendJobRecordingScope>(wire).is_err());
}

#[test]
fn child_binding_requires_same_journal_identity_cas_and_finished_previous_owner() {
    let store = store();
    reserve(&store, "host");
    child(&store, "child-a");
    let linked = store
        .set_backend_job_recording_child("w", "host", None, Some("child-a"))
        .unwrap();
    assert_eq!(linked.current_child_job_id.as_deref(), Some("child-a"));
    assert_eq!(
        store
            .set_backend_job_recording_child("w", "host", None, Some("child-a"))
            .unwrap(),
        linked
    );
    assert!(
        store
            .set_backend_job_recording_child("w", "host", Some("wrong"), None)
            .is_err()
    );
    assert!(
        store
            .set_backend_job_recording_child("w", "host", Some("child-a"), None)
            .is_err()
    );
    settle_child(&store, "child-a");
    store
        .set_backend_job_recording_child("w", "host", Some("child-a"), None)
        .unwrap();
    assert!(
        store
            .set_backend_job_recording_child("w", "host", None, Some("child-a"))
            .is_err()
    );
    let mut wrong = child_request("wrong-child");
    wrong.grants.subjektiv_journal.as_mut().unwrap().binding_id = "wrong-lifetime".into();
    store.reserve_backend_job("w", &wrong, NOW).unwrap();
    assert!(
        store
            .set_backend_job_recording_child("w", "host", None, Some("wrong-child"))
            .is_err()
    );
    settle_child(&store, "wrong-child");
    let mut wrong = child_request("wrong-journal");
    wrong.grants.subjektiv_journal.as_mut().unwrap().journal_id = "replacement-journal".into();
    wrong.serialization_key = Some(crate::backend_job::journal_serialization_key(
        "replacement-journal",
    ));
    store.reserve_backend_job("w", &wrong, NOW).unwrap();
    assert!(
        store
            .set_backend_job_recording_child("w", "host", None, Some("wrong-journal"))
            .is_err()
    );
    child(&store, "child-b");
    store
        .set_backend_job_recording_child("w", "host", None, Some("child-b"))
        .unwrap();
    assert_eq!(
        store
            .list_backend_job_recording_child_ids("w", "host", None, 1)
            .unwrap(),
        ["child-a"]
    );
    assert_eq!(
        store
            .list_backend_job_recording_child_ids("w", "host", Some("child-a"), 1)
            .unwrap(),
        ["child-b"]
    );
    reserve(&store, "host-other");
    assert!(
        store
            .set_backend_job_recording_child("w", "host-other", None, Some("child-b"))
            .is_err()
    );
    assert!(
        store
            .set_backend_job_recording_child("other", "host", None, Some("child-b"))
            .is_err()
    );
}

#[test]
fn host_finish_requires_canonical_child_terminal_outcome_and_all_attempt_cleanup() {
    let store = store();
    reserve(&store, "host");
    child(&store, "child");
    store
        .set_backend_job_recording_child("w", "host", None, Some("child"))
        .unwrap();
    let outcome = BackendJobRecordingOutcome::Completed {
        result: serde_json::json!({"recorded":true}),
    };
    assert!(
        store
            .finish_backend_job_recording("w", "host", &outcome, NOW)
            .is_err()
    );
    settle_child(&store, "child");
    store.with_conn(|conn| { conn.execute("UPDATE backend_job_attempts SET worker_cleanup_state='pending' WHERE workspace_id='w' AND job_id='child'",[])?; Ok(()) }).unwrap();
    assert!(
        store
            .finish_backend_job_recording("w", "host", &outcome, NOW)
            .is_err()
    );
    assert_eq!(
        store.get_backend_job("w", "host").unwrap().unwrap().state,
        BackendJobState::Pending
    );
    store.with_conn(|conn| { conn.execute("UPDATE backend_job_attempts SET worker_cleanup_state='completed' WHERE workspace_id='w' AND job_id='child'",[])?; Ok(()) }).unwrap();
    let saved = store
        .finish_backend_job_recording("w", "host", &outcome, NOW)
        .unwrap();
    assert_eq!(saved.state, BackendJobState::Completed);
    assert_eq!(
        store
            .finish_backend_job_recording("w", "host", &outcome, "later")
            .unwrap(),
        saved
    );
    assert!(
        store
            .list_active_backend_job_recording_ids("w", None, 100)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .finish_backend_job_recording(
                "w",
                "host",
                &BackendJobRecordingOutcome::Failed {
                    category: "failed".into(),
                    detail: "different".into()
                },
                NOW
            )
            .is_err()
    );
    assert!(
        store
            .set_backend_job_recording_child("w", "host", Some("child"), None)
            .is_err()
    );
    assert_eq!(
        store
            .get_backend_job_recording_chunks("w", "host", 0, 2)
            .unwrap()
            .chunks
            .len(),
        2
    );
}

#[test]
fn failed_host_retains_input_and_returns_exact_saved_failure_on_replay() {
    let store = store();
    reserve(&store, "host");
    let outcome = BackendJobRecordingOutcome::Failed {
        category: "capture_rejected".into(),
        detail: "Finite recording could not complete".into(),
    };
    let first = store
        .finish_backend_job_recording("w", "host", &outcome, NOW)
        .unwrap();
    assert_eq!(first.state, BackendJobState::Failed);
    assert_eq!(
        store
            .finish_backend_job_recording("w", "host", &outcome, "later")
            .unwrap(),
        first
    );
    assert_eq!(
        store
            .get_backend_job_recording_chunks("w", "host", 0, 2)
            .unwrap()
            .chunks
            .len(),
        2
    );
}

#[test]
fn schema_immutability_and_parent_cascade_preserve_ownership_boundaries() {
    let store = store();
    reserve(&store, "host");
    store.with_conn(|conn| {
        assert!(conn.execute("UPDATE backend_job_recording_inputs SET total_bytes=0 WHERE job_id='host'",[]).is_err());
        assert!(conn.execute("UPDATE backend_job_recording_chunks SET capture_json='{}' WHERE job_id='host'",[]).is_err());
        assert!(conn.execute("INSERT INTO backend_job_recording_chunks(workspace_id,job_id,chunk_index,capture_json,capture_digest) VALUES('w','missing',0,'{}',?1)",["0".repeat(64)]).is_err());
        conn.execute("DELETE FROM backend_jobs WHERE workspace_id='w' AND job_id='host'",[])?;
        assert_eq!(conn.query_row("SELECT count(*) FROM backend_job_recording_chunks",[],|r|r.get::<_,u64>(0))?,0);
        assert_eq!(conn.query_row("SELECT count(*) FROM backend_job_recording_inputs",[],|r|r.get::<_,u64>(0))?,0);
        assert_eq!(conn.query_row("SELECT count(*) FROM pragma_foreign_key_check",[],|r|r.get::<_,u64>(0))?,0);
        Ok(())
    }).unwrap();
}
