//! Lifecycle wire and storage contracts use the real store and isolated SQLite.
use super::*;
use serde_json::{Value, json};

fn invalid_states() -> Vec<Value> {
    vec![
        json!("open"),
        json!(null),
        json!([]),
        json!({}),
        json!({"kind":"open"}),
        json!({"kind":"unknown"}),
        json!({"kind":"closed","from":{"kind":"implementation"}}),
        json!({"kind":"implementation","started_by":"approval"}),
        json!({"kind":"implementation","extra":true}),
        json!({"kind":"integration"}),
        json!({"kind":"integration","started_by":null}),
        json!({"kind":"integration","started_by":3}),
        json!({"kind":"integration","started_by":"approval","completion":{}}),
        json!({"kind":"completed"}),
        json!({"kind":"completed","completion":null}),
        json!({"kind":"completed","completion":{"kind":"published","merge_event_id":"merge"}}),
        json!({"kind":"completed","completion":{"kind":"published","started_by":"approval"}}),
        json!({"kind":"completed","completion":{"kind":"published","started_by":"approval","merge_event_id":null}}),
        json!({"kind":"completed","completion":{"kind":"published","started_by":"approval","merge_event_id":"merge","extra":0}}),
        json!({"kind":"completed","completion":{"kind":"legacy","started_by":"approval"}}),
        json!({"kind":"completed","completion":{"kind":"legacy","merge_event_id":3}}),
        json!({"kind":"completed","completion":{"kind":"unknown"}}),
        json!({"kind":"completed","completion":{"kind":"legacy"},"started_by":"approval"}),
        json!({"kind":"canceled"}),
        json!({"kind":"canceled","from":null}),
        json!({"kind":"canceled","from":{"kind":"integration"}}),
        json!({"kind":"canceled","from":{"kind":"integration","started_by":null}}),
        json!({"kind":"canceled","from":{"kind":"implementation","started_by":"approval"}}),
        json!({"kind":"canceled","from":{"kind":"legacy","started_by":"approval"}}),
        json!({"kind":"canceled","from":{"kind":"unknown"}}),
        json!({"kind":"canceled","from":{"kind":"legacy"},"completion":{}}),
    ]
}

fn valid_states() -> Vec<Value> {
    vec![
        json!({"kind":"implementation"}),
        json!({"kind":"integration","started_by":"approval"}),
        json!({"kind":"completed","completion":{"kind":"published","started_by":"approval","merge_event_id":"merge"}}),
        json!({"kind":"completed","completion":{"kind":"legacy","merge_event_id":"merge"}}),
        json!({"kind":"completed","completion":{"kind":"legacy","merge_event_id":null}}),
        json!({"kind":"canceled","from":{"kind":"implementation"}}),
        json!({"kind":"canceled","from":{"kind":"integration","started_by":"approval"}}),
        json!({"kind":"canceled","from":{"kind":"legacy"}}),
    ]
}

#[test]
fn lifecycle_json_round_trips_payloads_and_rejects_unknown_or_inconsistent_fields() {
    for value in valid_states() {
        let state: MergeRequestState = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(state).unwrap(), value);
    }
    for value in invalid_states() {
        assert!(
            serde_json::from_value::<MergeRequestState>(value.clone()).is_err(),
            "{value}"
        );
    }
    let (_dir, store) = fixture();
    open(&store);
    let mr = store.get("W", "T").unwrap();
    // The DTO has one authoritative state and no phase/start mirror.
    let value = serde_json::to_value(mr).unwrap();
    assert!(value.get("phase").is_none());
    assert!(value.get("integration_started_by").is_none());
    for (key, extra) in [
        ("phase", json!("integration")),
        ("integration_started_by", json!("approval")),
    ] {
        let mut invalid = value.clone();
        invalid[key] = extra;
        assert!(serde_json::from_value::<MergeRequest>(invalid).is_err());
    }
    for value in ["open", "merged", "closed"] {
        let filter: MergeRequestStateFilter = serde_json::from_value(json!(value)).unwrap();
        assert_eq!(serde_json::to_value(filter).unwrap(), json!(value));
    }
    assert!(serde_json::from_value::<MergeRequestStateFilter>(json!("integration")).is_err());
}

#[test]
fn lifecycle_column_check_rejects_invalid_json_and_sum_shapes_independently_of_triggers() {
    let (dir, store) = fixture();
    open(&store);
    let c = Connection::open(dir.path().join("db")).unwrap();
    // Isolate the column CHECK: transition/evidence constraints have separate coverage.
    c.execute_batch(
        "DROP TRIGGER merge_request_lifecycle_transition;
        DROP TRIGGER merge_request_lifecycle_evidence_insert;
        DROP TRIGGER merge_request_lifecycle_evidence_update;",
    )
    .unwrap();
    for value in valid_states() {
        c.execute("UPDATE merge_requests SET state=?1", [value.to_string()])
            .unwrap();
        assert_eq!(
            serde_json::to_value(store.get("W", "T").unwrap().state).unwrap(),
            value
        );
    }
    for value in invalid_states().into_iter().map(|v| v.to_string()).chain([
        "not json".into(),
        r#"{"kind":"implementation","kind":"implementation"}"#.into(),
    ]) {
        let error = c
            .execute("UPDATE merge_requests SET state=?1", [&value])
            .unwrap_err();
        assert_eq!(
            error.sqlite_error().unwrap().extended_code,
            rusqlite::ffi::SQLITE_CONSTRAINT_CHECK,
            "{value}: {error}"
        );
    }
    for removed in ["integration_started_by", "phase", "lifecycle"] {
        assert!(
            c.prepare(&format!("SELECT {removed} FROM merge_requests"))
                .is_err()
        );
    }
}

#[test]
fn lifecycle_transitions_require_evidence_and_preserve_entry_and_terminal_provenance() {
    for close in [false, true] {
        let (dir, store) = fixture();
        open(&store);
        let c = Connection::open(dir.path().join("db")).unwrap();
        let update =
            |state: Value| c.execute("UPDATE merge_requests SET state=?1", [state.to_string()]);
        for state in [
            json!({"kind":"integration","started_by":"missing"}),
            json!({"kind":"completed","completion":{"kind":"legacy","merge_event_id":null}}),
            json!({"kind":"canceled","from":{"kind":"legacy"}}),
        ] {
            assert!(update(state).is_err());
        }
        let first = approve(&store, "source", "first");
        let later = approve(&store, "source", "later");
        for state in [
            json!({"kind":"implementation"}),
            json!({"kind":"integration","started_by":later.event_id}),
            json!({"kind":"canceled","from":{"kind":"implementation"}}),
            json!({"kind":"canceled","from":{"kind":"integration","started_by":later.event_id}}),
            json!({"kind":"completed","completion":{"kind":"published","started_by":first.event_id,"merge_event_id":"missing"}}),
        ] {
            assert!(update(state).is_err());
        }
        if close {
            update(
                json!({"kind":"canceled","from":{"kind":"integration","started_by":first.event_id}}),
            )
            .unwrap();
            assert!(store.get("W", "T").unwrap().state.is_canceled());
        } else {
            let input = super::integration::completion(&later);
            let merged = store.complete(input.clone()).unwrap();
            assert_eq!(store.complete(input).unwrap(), merged);
            assert_eq!(
                store.get("W", "T").unwrap().state,
                MergeRequestState::Completed {
                    completion: MergeRequestCompletion::Published {
                        started_by: first.event_id.clone(),
                        merge_event_id: merged.event_id
                    },
                }
            );
        }
        let terminal = store.get("W", "T").unwrap();
        assert_eq!(terminal.state.started_by(), Some(first.event_id.as_str()));
        assert!(!terminal.state.is_open());
        for state in valid_states() {
            assert!(update(state).is_err());
        }
        assert_eq!(store.get("W", "T").unwrap().state, terminal.state);
    }
}

#[test]
fn list_filters_group_both_open_phases_and_completed_without_conflating_state() {
    let (_dir, store) = fixture();
    open(&store);
    let list = |state| {
        store
            .list(
                "W",
                &MergeRequestListQuery {
                    state: Some(state),
                    limit: 10,
                    ..Default::default()
                },
            )
            .unwrap()
            .items
    };
    assert_eq!(
        list(MergeRequestStateFilter::Open)[0].state,
        MergeRequestState::Implementation {}
    );
    let review = approve(&store, "source", "review");
    assert_eq!(
        list(MergeRequestStateFilter::Open)[0].state,
        MergeRequestState::Integration {
            started_by: review.event_id.clone()
        }
    );
    assert!(list(MergeRequestStateFilter::Merged).is_empty());
    store
        .complete(super::integration::completion(&review))
        .unwrap();
    assert!(list(MergeRequestStateFilter::Open).is_empty());
    assert!(
        list(MergeRequestStateFilter::Merged)[0]
            .state
            .is_completed()
    );
}

#[test]
fn v13_terminal_migration_preserves_receipts_references_and_missing_result_diagnostics() {
    let (dir, store) = fixture();
    open(&store);
    let first = approve(&store, "source", "first");
    let input = super::integration::completion(&first);
    let merged = store.complete(input.clone()).unwrap();
    let c = Connection::open(dir.path().join("db")).unwrap();
    freeze_v13_schema(&c);
    c.execute_batch("PRAGMA foreign_keys=ON;
        CREATE TABLE retained_receipts(workspace_id TEXT,merge_request_id TEXT,payload TEXT,FOREIGN KEY(workspace_id,merge_request_id) REFERENCES merge_requests(workspace_id,merge_request_id));
        INSERT INTO retained_receipts VALUES('W','MR','historical receipt bytes');
        CREATE TRIGGER retain_parent BEFORE DELETE ON merge_requests BEGIN SELECT RAISE(ABORT,'retained parent'); END;
        INSERT INTO merge_requests(workspace_id,merge_request_id,repository_id,state,selector_to,created_at,updated_at)
            SELECT workspace_id,'missing','R','merged',selector_to,created_at,updated_at FROM merge_requests WHERE merge_request_id='MR';
        INSERT INTO merge_requests(workspace_id,merge_request_id,repository_id,state,selector_to,created_at,updated_at)
            SELECT workspace_id,'closed','R','closed',selector_to,created_at,updated_at FROM merge_requests WHERE merge_request_id='MR';").unwrap();
    let events = |c: &Connection| {
        c.prepare("SELECT payload_json FROM merge_request_thread_events ORDER BY sequence")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let before = events(&c);
    migrate(&c).unwrap();
    assert_eq!(events(&c), before);
    assert_eq!(
        store.get_by_id("W", "MR").unwrap().state,
        MergeRequestState::Completed {
            completion: MergeRequestCompletion::Legacy {
                merge_event_id: Some(merged.event_id.clone())
            }
        }
    );
    assert_eq!(store.complete(input).unwrap(), merged);
    let missing = store.get_by_id("W", "missing").unwrap();
    assert_eq!(
        missing.state,
        MergeRequestState::Completed {
            completion: MergeRequestCompletion::Legacy {
                merge_event_id: None
            }
        }
    );
    assert_eq!(
        missing.merged_result(),
        Err(MergeRequestEvidenceError::MergeResultMissing)
    );
    assert_eq!(
        store.get_by_id("W", "closed").unwrap().state,
        MergeRequestState::Canceled {
            from: MergeRequestCancellationOrigin::Legacy {}
        }
    );
    assert_eq!(
        c.query_row("SELECT payload FROM retained_receipts", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "historical receipt bytes"
    );
    assert!(
        !c.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    assert!(
        c.execute("DELETE FROM merge_requests WHERE merge_request_id='MR'", [])
            .is_err()
    );
}
