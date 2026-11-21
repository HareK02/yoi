// Store component coverage. Domain deletion proof and Runtime resource shutdown
// are supplied by their owners; these tests do not claim Server wiring coverage.
mod result_application_tests {
    use super::*;
    use crate::backend_job::{JobInputRetirementReason, JobResultApplicationDisposition::*};

    const WORKSPACE: &str = "resource-workspace";
    const LATER: &str = "2026-09-02T00:00:00Z";

    fn current(
        store: &SqliteWorkspaceStore,
        expected: &BackendJobAttemptRecord,
    ) -> BackendJobAttemptRecord {
        store
            .get_backend_job_attempt(WORKSPACE, &expected.job_id, &expected.attempt_id)
            .unwrap()
            .unwrap()
    }

    fn settle(
        store: &SqliteWorkspaceStore,
        expected: &BackendJobAttemptRecord,
        intent: &str,
        disposition: JobResultApplicationDisposition,
    ) -> Result<BackendJobResultApplicationReceipt> {
        store.record_backend_job_result_application(
            WORKSPACE,
            expected,
            intent,
            disposition,
            RESOURCE_JOB_NOW,
        )
    }

    fn accept(
        store: &SqliteWorkspaceStore,
        reservation: &BackendJobReservation,
    ) -> BackendJobResultAcceptance {
        let worker = bind_resource_job_worker(store, reservation, &reservation.job.request.job_id);
        store
            .accept_backend_job_result(
                WORKSPACE,
                &worker,
                &BackendJobResultSubmission {
                    job_id: reservation.job.request.job_id.clone(),
                    attempt_id: reservation.attempt.attempt_id.clone(),
                    input_digest: reservation.attempt.input_digest.clone(),
                    result: serde_json::json!({"accepted": true}),
                },
                RESOURCE_JOB_NOW,
            )
            .unwrap()
    }

    fn assert_only_application_changed(
        before: &BackendJobAttemptRecord,
        after: &BackendJobAttemptRecord,
    ) {
        let mut without_application = after.clone();
        without_application.result_application_disposition = before.result_application_disposition;
        without_application.result_application_recorded_at =
            before.result_application_recorded_at.clone();
        assert_eq!(&without_application, before);
    }

    #[test]
    fn result_application_vocabulary_is_backend_owned_and_strict() {
        for disposition in [Applied, NotRequired] {
            assert_eq!(
                JobResultApplicationDisposition::parse(disposition.as_str()).unwrap(),
                disposition
            );
            assert_eq!(
                serde_json::from_value::<JobResultApplicationDisposition>(serde_json::json!(
                    disposition.as_str()
                ))
                .unwrap(),
                disposition
            );
        }
        assert!(JobResultApplicationDisposition::parse("failed").is_err());
        assert!(serde_json::from_str::<JobResultApplicationDisposition>("\"completed\"").is_err());
    }

    #[tokio::test]
    async fn result_application_applied_requires_acceptance_preserves_state_and_replays_after_cleanup_reopen()
     {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("applied.sqlite");
        let store = SqliteWorkspaceStore::open_api_fixture(&path).unwrap();
        seed_resource_job_workspace(&store).await;
        let request = resource_job_request("applied", "subject", "candidate");
        let reservation = store
            .reserve_backend_job(WORKSPACE, &request, RESOURCE_JOB_NOW)
            .unwrap();
        let intent = &reservation.job.intent_fingerprint;
        assert!(settle(&store, &reservation.attempt, intent, Applied).is_err());
        let accepted = accept(&store, &reservation);
        let mut wrong_result = accepted.attempt.clone();
        wrong_result.result_digest = Some("wrong-result".into());
        assert!(settle(&store, &wrong_result, intent, Applied).is_err());
        let receipt = settle(&store, &accepted.attempt, intent, Applied).unwrap();
        assert!(!receipt.replayed);
        assert_eq!(receipt.disposition, Applied);
        let after = current(&store, &accepted.attempt);
        assert_only_application_changed(&accepted.attempt, &after);
        assert_eq!(after.result_application_disposition, Some(Applied));
        assert_eq!(
            after.result_application_recorded_at.as_deref(),
            Some(RESOURCE_JOB_NOW)
        );
        assert_eq!(
            store
                .get_backend_job(WORKSPACE, "applied")
                .unwrap()
                .unwrap(),
            accepted.job
        );
        // Real store cleanup/finalization and input retirement, with a fixture
        // execution fence (not an actual Runtime stop).
        finish_resource_job_cleanup(&store, &reservation);
        store
            .reclaim_backend_job_input(
                WORKSPACE,
                "applied",
                &reservation.job.input_digest,
                JobInputRetirementReason::Completed,
            )
            .unwrap();
        drop(store);
        let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        let replay = reopened
            .record_backend_job_result_application(
                WORKSPACE,
                &accepted.attempt,
                intent,
                Applied,
                LATER,
            )
            .unwrap();
        let mut original = receipt;
        original.replayed = true;
        assert_eq!(replay, original);
        assert!(settle(&reopened, &wrong_result, intent, Applied).is_err());
        assert!(settle(&reopened, &accepted.attempt, intent, NotRequired).is_err());
        assert_eq!(
            reopened
                .get_backend_job(WORKSPACE, "applied")
                .unwrap()
                .unwrap()
                .state,
            BackendJobState::Completed
        );
    }

    #[tokio::test]
    async fn result_application_not_required_does_not_finish_non_journal_execution_or_resources() {
        let store = SqliteWorkspaceStore::in_memory_fixture().unwrap();
        seed_resource_job_workspace(&store).await;
        // Consolidation is deliberately not a Journal consumer.
        let request = resource_job_request("deleted-target", "deleted-subject", "candidate");
        assert!(request.grants.subjektiv_journal.is_none());
        let reservation = store
            .reserve_backend_job(WORKSPACE, &request, RESOURCE_JOB_NOW)
            .unwrap();
        bind_resource_job_worker(&store, &reservation, "deleted-target");
        // Canonical host deletion inventory, not a missing registry/map entry.
        // Its preparation fences execution as Failed/subject_deleted while the
        // resources remain owned. Application must not change those facts.
        store
            .prepare_subject_deletion(WORKSPACE, "deleted-subject", "delete-target", &[])
            .unwrap();
        let before = current(&store, &reservation.attempt);
        let job = store
            .get_backend_job(WORKSPACE, "deleted-target")
            .unwrap()
            .unwrap();
        assert!(before.worker.is_some());
        assert_eq!(job.state, BackendJobState::Failed);
        assert_eq!(before.state, BackendJobAttemptState::Failed);
        assert_eq!(
            before.worker_cleanup_state,
            Some(BackendJobWorkerCleanupState::Pending)
        );
        assert!(
            store
                .with_conn(|conn| subject_deletion_store::require_job_admission(
                    conn,
                    WORKSPACE,
                    &job.request
                ))
                .is_err()
        );
        assert!(settle(&store, &before, &job.intent_fingerprint, Applied).is_err());
        let receipt = settle(&store, &before, &job.intent_fingerprint, NotRequired).unwrap();
        assert_eq!(receipt.disposition, NotRequired);
        let after = current(&store, &before);
        assert_only_application_changed(&before, &after);
        assert_eq!(
            store
                .get_backend_job(WORKSPACE, "deleted-target")
                .unwrap()
                .unwrap(),
            job
        );
        assert!(
            store
                .with_conn(|conn| backend_job_has_unfinished_cleanup(
                    conn,
                    WORKSPACE,
                    &before.job_id
                ))
                .unwrap()
        );
        assert_eq!(
            store
                .finalize_backend_job(WORKSPACE, &before.job_id, LATER)
                .unwrap(),
            job
        );
        assert_eq!(current(&store, &before), after);
        assert_eq!(
            store
                .get_backend_job(WORKSPACE, &before.job_id)
                .unwrap()
                .unwrap(),
            job
        );
        assert!(
            store
                .with_conn(|conn| backend_job_has_unfinished_cleanup(
                    conn,
                    WORKSPACE,
                    &before.job_id
                ))
                .unwrap()
        );
        assert!(
            store
                .reclaim_backend_job_input(
                    WORKSPACE,
                    &before.job_id,
                    &before.input_digest,
                    JobInputRetirementReason::Completed
                )
                .is_err()
        );
        let replay = settle(&store, &before, &job.intent_fingerprint, NotRequired).unwrap();
        assert!(replay.replayed);
        assert!(settle(&store, &before, &job.intent_fingerprint, Applied).is_err());
    }

    #[tokio::test]
    async fn result_application_fences_identity_input_worker_run_intent_and_noncurrent_attempt_on_replay()
     {
        let store = SqliteWorkspaceStore::in_memory_fixture().unwrap();
        seed_resource_job_workspace(&store).await;
        let request = resource_job_request("fenced", "subject", "candidate");
        let reservation = store
            .reserve_backend_job(WORKSPACE, &request, RESOURCE_JOB_NOW)
            .unwrap();
        let worker = bind_resource_job_worker(&store, &reservation, "fenced");
        let bound = current(&store, &reservation.attempt);
        let intent = &reservation.job.intent_fingerprint;
        let mut bad = Vec::new();
        let mut changed = bound.clone();
        changed.workspace_id = "other".into();
        bad.push(changed);
        let mut changed = bound.clone();
        changed.job_id = "other".into();
        bad.push(changed);
        let mut changed = bound.clone();
        changed.attempt_id = "other".into();
        bad.push(changed);
        let mut changed = bound.clone();
        changed.attempt += 1;
        bad.push(changed);
        let mut changed = bound.clone();
        changed.input_digest = "other".into();
        bad.push(changed);
        let mut changed = bound.clone();
        changed.worker = Some(RuntimeWorkerRef::new("other-runtime", &worker.worker_id));
        bad.push(changed);
        let mut changed = bound.clone();
        changed.worker = Some(RuntimeWorkerRef::new(&worker.runtime_id, "other-worker"));
        bad.push(changed);
        let mut changed = bound.clone();
        changed.worker = None;
        bad.push(changed);
        let mut changed = bound.clone();
        changed.runtime_run_id = Some("other-run".into());
        bad.push(changed);
        let mut changed = bound.clone();
        changed.runtime_run_id = None;
        bad.push(changed);
        bad.push(reservation.attempt.clone());
        for expected in &bad {
            assert!(settle(&store, expected, intent, NotRequired).is_err());
        }
        assert!(settle(&store, &bound, "wrong-intent", NotRequired).is_err());
        assert!(
            current(&store, &bound)
                .result_application_disposition
                .is_none()
        );
        // Store boundary test: the host consumer is responsible for proving
        // target deletion; this test injects its decision directly.
        settle(&store, &bound, intent, NotRequired).unwrap();
        for expected in &bad {
            assert!(settle(&store, expected, intent, NotRequired).is_err());
        }
        assert!(settle(&store, &bound, "wrong-intent", NotRequired).is_err());
        store
            .finish_backend_job_attempt(
                WORKSPACE,
                &bound.job_id,
                &bound.attempt_id,
                BackendJobAttemptState::Failed,
                "test_failure",
                "fixture",
                RESOURCE_JOB_NOW,
            )
            .unwrap();
        finish_resource_job_cleanup(&store, &reservation);
        let next = store
            .reserve_backend_job_retry(WORKSPACE, &bound.job_id, LATER)
            .unwrap();
        assert_eq!(next.attempt.attempt, 2);
        assert!(next.attempt.result_application_disposition.is_none());
        assert!(
            settle(&store, &bound, intent, NotRequired).is_err(),
            "even an identical saved receipt is stale after retry"
        );
    }

    #[tokio::test]
    async fn result_application_unbound_settlement_is_not_allocation_preclusion() {
        let store = SqliteWorkspaceStore::in_memory_fixture().unwrap();
        seed_resource_job_workspace(&store).await;
        let request = resource_job_request("unbound", "subject", "candidate");
        let reservation = store
            .reserve_backend_job(WORKSPACE, &request, RESOURCE_JOB_NOW)
            .unwrap();
        settle(
            &store,
            &reservation.attempt,
            &reservation.job.intent_fingerprint,
            NotRequired,
        )
        .unwrap();
        let after = current(&store, &reservation.attempt);
        assert_only_application_changed(&reservation.attempt, &after);
        assert!(after.allocation_precluded_at.is_none());
        assert_eq!(after.state, BackendJobAttemptState::Reserved);
        assert!(
            store
                .finalize_backend_job(WORKSPACE, &after.job_id, LATER)
                .is_err()
        );
    }

    #[tokio::test]
    async fn result_application_update_failure_rolls_back_and_sql_cannot_overwrite_settlement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollback.sqlite");
        let store = SqliteWorkspaceStore::open_api_fixture(&path).unwrap();
        seed_resource_job_workspace(&store).await;
        let request = resource_job_request("rollback", "subject", "candidate");
        let reservation = store
            .reserve_backend_job(WORKSPACE, &request, RESOURCE_JOB_NOW)
            .unwrap();
        store.with_conn(|conn| { conn.execute_batch("CREATE TRIGGER application_fault AFTER UPDATE OF result_application_disposition ON backend_job_attempts BEGIN SELECT RAISE(ABORT,'injected write failure'); END;")?; Ok(()) }).unwrap();
        assert!(
            settle(
                &store,
                &reservation.attempt,
                &reservation.job.intent_fingerprint,
                NotRequired
            )
            .is_err()
        );
        assert_eq!(current(&store, &reservation.attempt), reservation.attempt);
        store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER application_fault")?;
                Ok(())
            })
            .unwrap();
        drop(store);
        let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert_eq!(
            current(&reopened, &reservation.attempt),
            reservation.attempt
        );
        settle(
            &reopened,
            &reservation.attempt,
            &reservation.job.intent_fingerprint,
            NotRequired,
        )
        .unwrap();
        reopened.with_conn(|conn| {
            for sql in [
                "UPDATE backend_job_attempts SET result_application_disposition='applied'",
                "UPDATE backend_job_attempts SET result_application_disposition=NULL,result_application_recorded_at=NULL",
                "UPDATE backend_job_attempts SET result_application_recorded_at='later'",
                "UPDATE backend_job_attempts SET result_application_disposition='failed'",
            ] { assert!(conn.execute(sql, []).is_err(), "{sql}"); }
            Ok(())
        }).unwrap();
        assert!(
            settle(
                &reopened,
                &reservation.attempt,
                &reservation.job.intent_fingerprint,
                NotRequired
            )
            .unwrap()
            .replayed
        );
    }

    // Copy only the historical columns. This builds real schema138 from the
    // retained migration chain and seeds live Job data without downgrading DDL.
    fn copy_old_rows(source: &Connection, target: &Connection, table: &str) {
        let names = table_columns(target, table)
            .unwrap()
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(",");
        let mut stmt = source
            .prepare(&format!("SELECT {names} FROM {table}"))
            .unwrap();
        let count = stmt.column_count();
        let rows = stmt
            .query_map([], |row| {
                (0..count)
                    .map(|i| row.get::<_, rusqlite::types::Value>(i))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let placeholders = vec!["?"; count].join(",");
        for row in rows {
            target
                .execute(
                    &format!("INSERT INTO {table} ({names}) VALUES ({placeholders})"),
                    rusqlite::params_from_iter(row),
                )
                .unwrap();
        }
    }

    fn application_signature(conn: &Connection) -> (Vec<Vec<String>>, String) {
        let mut stmt = conn.prepare("SELECT name,type,\"notnull\",coalesce(dflt_value,''),pk FROM pragma_table_info('backend_job_attempts') WHERE name LIKE 'result_application_%' ORDER BY name").unwrap();
        let columns = stmt
            .query_map([], |r| {
                Ok(vec![
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2)?.to_string(),
                    r.get(3)?,
                    r.get::<_, i64>(4)?.to_string(),
                ])
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let trigger = conn.query_row("SELECT sql FROM sqlite_schema WHERE name='backend_job_result_application_immutable'", [], |r| r.get(0)).unwrap();
        (columns, trigger)
    }

    #[tokio::test]
    async fn result_application_schema139_preserves_old_attempts_and_matches_fresh_reopens() {
        let fresh = SqliteWorkspaceStore::in_memory_fixture().unwrap();
        seed_resource_job_workspace(&fresh).await;
        let request = resource_job_request("migration", "subject", "candidate");
        let reservation = fresh
            .reserve_backend_job(WORKSPACE, &request, RESOURCE_JOB_NOW)
            .unwrap();
        let accepted = accept(&fresh, &reservation);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old138.sqlite");
        prepare_retained_schema(&path, 138);
        let old = Connection::open(&path).unwrap();
        configure_sqlite(&old).unwrap();
        fresh
            .with_conn(|source| {
                for table in [
                    "accounts",
                    "workspaces",
                    "backend_jobs",
                    "backend_job_attempts",
                ] {
                    copy_old_rows(source, &old, table);
                }
                Ok(())
            })
            .unwrap();
        let job_before = stored_fixture_rows(&old, "backend_jobs");
        let attempt_before = stored_fixture_rows(&old, "backend_job_attempts");
        assert!(
            !column_exists(
                &old,
                "backend_job_attempts",
                "result_application_disposition"
            )
            .unwrap()
        );
        drop(old);
        let upgraded = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        upgraded
            .with_conn(|conn| {
                assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
                assert_eq!(stored_fixture_rows(conn, "backend_jobs"), job_before);
                let after = stored_fixture_rows(conn, "backend_job_attempts");
                for (before, after) in attempt_before.iter().zip(&after) {
                    assert_eq!(&after[..before.len()], before);
                    assert_eq!(
                        &after[before.len()..],
                        &[rusqlite::types::Value::Null, rusqlite::types::Value::Null]
                    );
                }
                assert_eq!(after.len(), attempt_before.len());
                assert_eq!(
                    application_signature(conn),
                    fresh.with_conn(|conn| Ok(application_signature(conn)))?
                );
                assert_eq!(
                    conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                        .get::<_, i64>(
                        0
                    ))?,
                    0
                );
                Ok(())
            })
            .unwrap();
        assert_eq!(current(&upgraded, &accepted.attempt), accepted.attempt);
        settle(
            &upgraded,
            &accepted.attempt,
            &accepted.job.intent_fingerprint,
            Applied,
        )
        .unwrap();
        drop(upgraded);
        let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        assert!(
            settle(
                &reopened,
                &accepted.attempt,
                &accepted.job.intent_fingerprint,
                Applied
            )
            .unwrap()
            .replayed
        );
    }

    #[test]
    fn result_application_schema139_ddl_and_marker_rollback_together() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old138.sqlite");
        prepare_retained_schema(&path, 138);
        let old = Connection::open(&path).unwrap();
        configure_sqlite(&old).unwrap();
        old.execute_batch("CREATE TRIGGER backend_job_result_application_immutable BEFORE UPDATE ON backend_job_attempts BEGIN SELECT RAISE(ABORT,'collision'); END;").unwrap();
        assert!(backend_job_result_application::migrate(&old).is_err());
        assert_eq!(current_schema_version(&old).unwrap(), 138);
        assert!(
            !column_exists(
                &old,
                "backend_job_attempts",
                "result_application_disposition"
            )
            .unwrap()
        );
        assert!(
            !column_exists(
                &old,
                "backend_job_attempts",
                "result_application_recorded_at"
            )
            .unwrap()
        );
        old.execute_batch("DROP TRIGGER backend_job_result_application_immutable")
            .unwrap();
        // A marker write failure after both ALTERs must roll back the same way.
        old.execute_batch("CREATE TRIGGER application_marker_fault BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=139 BEGIN SELECT RAISE(ABORT,'marker fault'); END;").unwrap();
        assert!(backend_job_result_application::migrate(&old).is_err());
        assert_eq!(current_schema_version(&old).unwrap(), 138);
        assert!(
            !column_exists(
                &old,
                "backend_job_attempts",
                "result_application_disposition"
            )
            .unwrap()
        );
        old.execute_batch("DROP TRIGGER application_marker_fault")
            .unwrap();
        drop(old);
        let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        reopened
            .with_conn(|conn| {
                assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
                Ok(())
            })
            .unwrap();
    }
}
