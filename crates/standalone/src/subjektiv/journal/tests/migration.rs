//! The stored contract is shared by schema migration and current Host admission.
//! Temporary Job databases use a real Journal input/grant from the local Host fixture.
use super::*;
use crate::job_store::{JobStore, JobStoreError};

#[test]
fn migration_keeps_current_journal_inputs_and_retired_receipts_but_removes_old_contracts() {
    let fixture = Fixture::new("original immutable Journal input");
    let (_, _, current) = fixture.adapter();
    for case in [
        "current",
        "reclaimed",
        "erased",
        "missing-writer",
        "missing-operations",
        "origins",
        "descriptor-only",
        "old-capture",
        "mismatched-ranges",
    ] {
        let path = fixture
            ._temp
            .path()
            .join(format!("contract-{case}.sqlite3"));
        let mut store = JobStore::open(&path).unwrap();
        let mut request = current.request.clone();
        let mut grant = current.domain_grant.clone().unwrap();
        match case {
            "missing-writer" => {
                grant.as_object_mut().unwrap().remove("writer_binding_id");
            }
            "missing-operations" => {
                grant.as_object_mut().unwrap().remove("operations");
            }
            "origins" => {
                grant["origins"] = serde_json::json!([]);
            }
            "descriptor-only" => {
                request.input = serde_json::json!({"selected_ranges":grant["ranges"]});
            }
            "old-capture" => {
                request.input["capture"]["version"] = serde_json::json!(0);
            }
            "mismatched-ranges" => {
                grant["ranges"][0]["bytes"]["end"] = serde_json::json!(999999);
            }
            _ => {}
        }
        let mut saved = store
            .reserve_granted(request.clone(), Some(grant.clone()))
            .unwrap();
        if case == "reclaimed" {
            let running = store
                .dispatch(&request.job_id, &saved.attempt.attempt_id, 8)
                .unwrap();
            store
                .accept(&job::JobResultSubmission {
                    job_id: request.job_id.clone(),
                    attempt_id: saved.attempt.attempt_id.clone(),
                    input_digest: saved.input_digest.clone(),
                    result: serde_json::json!({"stored":"Journal output"}),
                })
                .unwrap();
            store
                .confirm_execution_cleanup(
                    &request.job_id,
                    &running.executions[0],
                    "fixture-owned-run-joined",
                )
                .unwrap();
            store.ack(&request.job_id).unwrap();
            saved = store
                .reclaim_input(
                    &request.job_id,
                    &saved.input_digest,
                    crate::jobs::JobInputRetirementReason::Completed,
                )
                .unwrap();
        } else if case == "erased" {
            store.cancel(&request.job_id).unwrap();
            saved = store
                .reclaim_input(
                    &request.job_id,
                    &saved.input_digest,
                    crate::jobs::JobInputRetirementReason::ExplicitErasure,
                )
                .unwrap();
        }
        let keep = matches!(case, "current" | "reclaimed" | "erased");
        assert_eq!(
            LocalSubjectHost::validate_stored_journal_job(
                &saved.request,
                &grant,
                saved.input_disposition == JobInputDisposition::Available
            )
            .is_ok(),
            keep,
            "{case}"
        );
        if !keep {
            assert!(fixture.host().journal_grant(&saved).is_err(), "{case}");
        }
        drop(store);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 9).unwrap();
        drop(conn);
        let migrated = JobStore::open(&path).unwrap();
        if keep {
            assert_eq!(migrated.get(&request.job_id).unwrap(), saved, "{case}");
        } else {
            assert!(
                matches!(
                    migrated.get(&request.job_id),
                    Err(JobStoreError::NotFound(_))
                ),
                "{case}"
            );
        }
        drop(migrated);
        let reopened = JobStore::open(&path).unwrap();
        assert_eq!(reopened.get(&request.job_id).is_ok(), keep, "{case}");
    }
}

#[test]
fn frozen_operations_never_default_to_all_current_journal_authority() {
    let fixture = Fixture::new("current scope");
    let (_, _, saved) = fixture.adapter();
    let mut value = saved.domain_grant.clone().unwrap();
    value["operations"] = serde_json::json!(["journal.job.input.read"]);
    let restricted = LocalJournalGrant::validate_stored(&saved.request, &value, true).unwrap();
    assert!(
        restricted
            .authorize_operation("journal.job.input.read")
            .is_ok()
    );
    assert!(
        restricted
            .authorize_operation("journal.job.input.supply")
            .is_err()
    );
    value["operations"] = serde_json::json!([]);
    let empty = LocalJournalGrant::validate_stored(&saved.request, &value, true).unwrap();
    assert!(empty.authorize_operation("journal.job.input.read").is_err());
    for field in ["writer_binding_id", "operations"] {
        let mut missing = saved.domain_grant.clone().unwrap();
        missing.as_object_mut().unwrap().remove(field);
        assert!(LocalJournalGrant::validate_stored(&saved.request, &missing, true).is_err());
    }
}
