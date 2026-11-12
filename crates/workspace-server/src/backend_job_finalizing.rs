//! Job completion belongs to the resource owner, not the result callback.
//! Attempt outcomes remain immutable while Finalizing owns the unfinished release.
use super::*;

pub(super) const MIGRATION_NAME: &str = "Backend Job completion after owned resource release";
const OLD_STATE: &str = "CHECK (state IN ('pending', 'completed', 'failed'))";
const NEW_STATE: &str = "CHECK (state IN ('pending', 'finalizing', 'completed', 'failed'))";
const OLD_RESULT: &str = "CHECK ((state = 'completed') = (result_json IS NOT NULL))";
const NEW_RESULT: &str = "CHECK ((state = 'completed' AND result_json IS NOT NULL) OR (state = 'finalizing') OR (state IN ('pending','failed') AND result_json IS NULL))";

pub(super) fn current_schema(sql: &str) -> String {
    // Only the Job CHECK changes; attempt Completed continues to mean result accepted.
    sql.replacen(OLD_STATE, NEW_STATE, 1)
        .replacen(OLD_RESULT, NEW_RESULT, 1)
}

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if !conn.is_autocommit() || current_schema_version(conn)? != 133 {
        return Err(Error::Store(
            "Job finalization requires the schema-133 boundary".into(),
        ));
    }
    storage_formats::verify_history(conn, 133)?;
    let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let result = (|| {
        let tx = conn.unchecked_transaction()?;
        let ddl: String = tx.query_row(
            "SELECT sql FROM sqlite_schema WHERE name='backend_jobs'",
            [],
            |r| r.get(0),
        )?;
        if ddl.matches(OLD_STATE).count() != 1 || ddl.matches(OLD_RESULT).count() != 1 {
            return Err(Error::Store("Unrecognized Job completion CHECK".into()));
        }
        let programs = tx.prepare("SELECT sql FROM sqlite_schema WHERE tbl_name='backend_jobs' AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name")?
            .query_map([], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        tx.execute_batch("CREATE TEMP TABLE __job_completion AS SELECT * FROM backend_jobs; DROP TABLE backend_jobs;")?;
        tx.execute_batch(&current_schema(&ddl))?;
        tx.execute_batch("INSERT INTO backend_jobs SELECT * FROM temp.__job_completion; DROP TABLE temp.__job_completion;")?;
        for sql in programs {
            tx.execute_batch(&sql)?;
        }
        // Saved cleanup completion is sufficient at this one-time boundary. Do not
        // invent new fences, reopen settled history, or turn migration into cleanup.
        tx.execute_batch("UPDATE backend_jobs AS j SET state='finalizing',completed_at=NULL WHERE state IN ('completed','failed') AND EXISTS(SELECT 1 FROM backend_job_attempts a WHERE a.workspace_id=j.workspace_id AND a.job_id=j.job_id AND a.worker_cleanup_state IS NOT 'completed');")?;
        tx.execute(
            "INSERT INTO __yoi_schema_migrations(version,name) VALUES(134,?1)",
            [MIGRATION_NAME],
        )?;
        let violations: i64 =
            tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })?;
        if violations != 0 {
            return Err(Error::Store(
                "Job finalization migration violates foreign keys".into(),
            ));
        }
        tx.commit()?;
        Ok(())
    })();
    let restore = conn
        .execute_batch(if foreign_keys {
            "PRAGMA foreign_keys=ON;"
        } else {
            "PRAGMA foreign_keys=OFF;"
        })
        .map_err(Error::from);
    result.and(restore)
}

/// Caller owns the transaction. Cleanup completion confirms execution fencing;
/// do not require unavailable receipts again for already released historical owners.
/// Successful Journal input retires before completion, while failed input remains
/// the durable same-Job recovery responsibility rather than being silently discarded.
pub(super) fn settle(
    conn: &Connection,
    workspace: &str,
    id: &str,
    now: &str,
) -> Result<BackendJobRecord> {
    let job = read_backend_job(conn, workspace, id)?
        .ok_or_else(|| Error::Store("Finalizing Job disappeared".into()))?;
    if matches!(
        job.state,
        BackendJobState::Completed | BackendJobState::Failed
    ) {
        return Ok(job);
    }
    if job.state != BackendJobState::Finalizing {
        return Err(Error::RepositoryConflict(
            "Job outcome has not been accepted".into(),
        ));
    }
    let attempts = backend_job_recovery_store::history(conn, workspace, id)?;
    let current = attempts
        .iter()
        .find(|a| a.attempt == job.current_attempt)
        .ok_or_else(|| Error::Store("Finalizing attempt missing".into()))?;
    for attempt in &attempts {
        if !attempt.state.terminal()
            || attempt.worker_cleanup_state != Some(BackendJobWorkerCleanupState::Completed)
        {
            return Err(Error::RepositoryConflict(
                "Job execution, Host effects or resource release remains unconfirmed".into(),
            ));
        }
    }
    if current.state == BackendJobAttemptState::Completed
        && job.request.grants.subjektiv_journal.is_some()
        && job.input_disposition == crate::backend_job::JobInputDisposition::Available
    {
        return Err(Error::RepositoryConflict(
            "Journal input retirement remains owned by the Job".into(),
        ));
    }
    let state = match current.state {
        BackendJobAttemptState::Completed => BackendJobState::Completed,
        BackendJobAttemptState::Failed => BackendJobState::Failed,
        _ => {
            return Err(Error::RepositoryConflict(
                "Job result remains unresolved".into(),
            ));
        }
    };
    conn.execute("UPDATE backend_jobs SET state=?3,completed_at=?4,updated_at=?4 WHERE workspace_id=?1 AND job_id=?2 AND state='finalizing'", params![workspace,id,state.as_str(),now])?;
    if state == BackendJobState::Completed {
        notification_routing::jobs::capture(conn, workspace, id, &current.attempt_id)?;
    }
    read_backend_job(conn, workspace, id)?
        .ok_or_else(|| Error::Store("Completed Job disappeared".into()))
}

#[cfg(test)]
mod tests {
    //! Real SQLite migration and rollback; synthetic execution rows do not claim
    //! Runtime termination. Server integration covers the actual cleanup owner.
    use super::*;

    fn seed(conn: &Connection) {
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('w','owner','Workspace','active','1','1');").unwrap();
        for (id, state, cleanup) in [
            ("success", "completed", "completed"),
            ("failure", "failed", "completed"),
            ("release-success", "completed", "pending"),
            ("release-failure", "failed", "executing"),
        ] {
            let result = (state == "completed").then_some("{\"accepted\":true}");
            let request = serde_json::json!({"job_id":id,"purpose":"fixture","input_ref":"saved","input":{"retained":true},"instruction":"Use saved input","profile":"builtin:backend-job"});
            conn.execute("INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,result_json,result_digest,created_at,updated_at,completed_at) VALUES('w',?1,'fixture','digest','saved',?2,'intent',?3,1,?4,?5,'created','accepted','accepted')", params![id,request.to_string(),state,result,result.map(|_|"result-digest")]).unwrap();
            conn.execute("INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,input_digest,state,deadline_at,result_json,result_digest,worker_cleanup_state,created_at,updated_at,completed_at) VALUES('w',?1,?1,1,'digest',?2,'deadline',?3,?4,?5,'created','accepted','accepted')", params![id,state,result,result.map(|_|"result-digest"),cleanup]).unwrap();
        }
    }

    #[test]
    fn migration_keeps_settled_success_and_failure_without_new_fences_and_reopens_only_unfinished_release()
     {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite");
        super::super::tests::prepare_retained_schema(&path, 133);
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        seed(&conn);
        migrate(&conn).unwrap();
        storage_formats::verify_history(&conn, 134).unwrap();
        assert_eq!(
            read_backend_job(&conn, "w", "success")
                .unwrap()
                .unwrap()
                .state,
            BackendJobState::Completed
        );
        assert_eq!(
            read_backend_job(&conn, "w", "failure")
                .unwrap()
                .unwrap()
                .state,
            BackendJobState::Failed
        );
        for id in ["release-success", "release-failure"] {
            let job = read_backend_job(&conn, "w", id).unwrap().unwrap();
            assert_eq!(job.state, BackendJobState::Finalizing);
            assert!(job.completed_at.is_none());
            assert_eq!(job.result.is_some(), id == "release-success");
            assert_eq!(job.request.input, serde_json::json!({"retained":true}));
        }
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM backend_job_execution_fences",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        drop(conn);
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        let ids: Vec<_> = store
            .list_backend_job_worker_cleanups("w", None, 100)
            .unwrap()
            .into_iter()
            .map(|a| a.job_id)
            .collect();
        assert_eq!(ids, ["release-failure", "release-success"]);
        assert!(
            store
                .finalize_backend_job("w", "release-success", "later")
                .is_err()
        );
        assert_eq!(
            store
                .finalize_backend_job("w", "success", "later")
                .unwrap()
                .completed_at
                .as_deref(),
            Some("accepted")
        );
    }

    fn assert_saved_journal_request_survives_migration(version: i64) {
        // Literal retained bytes, not a request regenerated by the current serializer.
        // binding_id identifies the source lifetime, not an active writer lease.
        const REQUEST: &str = r#"{
  "job_id": "retained-journal",
  "purpose": "subjektiv_journal",
  "input_ref": "journal:source-session",
  "input": { "text": "hello" },
  "instruction": "Record the retained public source",
  "profile": "builtin:subjektiv-journal-recording",
  "grants": {
    "subjektiv_journal": {
      "binding_id": "source-lifetime",
      "subject_id": "subject",
      "session_id": "source-session",
      "journal_id": "journal",
      "selected_ranges": [{
        "source": {
          "session_id": "source-session",
          "pointer": {
            "segment_id": "019a4ea8-0000-7000-8000-000000000001",
            "entry_ref": "E1"
          },
          "order": 0,
          "content_sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
          "total_bytes": 5
        },
        "bytes": { "start": 0, "end": 5 }
      }]
    }
  },
  "serialization_key": "subjektiv-journal:journal",
  "limits": {
    "max_concurrent_jobs": 1,
    "timeout_seconds": 300,
    "max_result_bytes": 1024,
    "max_attempts": 3
  }
}"#;
        let request: BackendJobRequest = serde_json::from_str(REQUEST).unwrap();
        request.validate().unwrap();
        let digest = request.input_digest().unwrap();
        let fingerprint = request.fingerprint().unwrap();
        let attempt_id = attempt_id(&request.job_id, 1);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("retained-journal.sqlite");
        super::super::tests::prepare_retained_schema(&path, version);
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('w','owner','Workspace','active','1','1');").unwrap();
        conn.execute(
            "INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,resource_key,created_at,updated_at) VALUES('w',?1,?2,?3,?4,?5,?6,'pending',1,?7,'2026-11-04T00:00:00Z','2026-11-04T00:00:00Z')",
            params![request.job_id, request.purpose, digest, request.input_ref, REQUEST, fingerprint, request.resource_key()],
        ).unwrap();
        conn.execute(
            "INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,input_digest,state,deadline_at,created_at,updated_at) VALUES('w',?1,?2,1,?3,'reserved','2026-11-04T00:05:00Z','2026-11-04T00:00:00Z','2026-11-04T00:00:00Z')",
            params![request.job_id, attempt_id, digest],
        ).unwrap();
        let before = read_backend_job(&conn, "w", &request.job_id)
            .unwrap()
            .unwrap();
        let before_attempt = read_backend_job_attempt(&conn, "w", &request.job_id, &attempt_id)
            .unwrap()
            .unwrap();
        assert_eq!(before.state, BackendJobState::Pending);
        assert_eq!(before_attempt.state, BackendJobAttemptState::Reserved);
        assert_eq!(before.load_executable_request().unwrap(), &request);

        let assert_preserved = |conn: &Connection| -> Result<()> {
            storage_formats::verify_history(conn, current_schema_version(conn)?)?;
            let (raw, resource_key): (String, Option<String>) = conn.query_row(
                "SELECT request_json,resource_key FROM backend_jobs WHERE workspace_id='w' AND job_id=?1",
                [&request.job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(raw, REQUEST);
            assert_eq!(resource_key, request.resource_key());
            let job = read_backend_job(conn, "w", &request.job_id)?.unwrap();
            assert_eq!(job, before);
            assert_eq!(job.request.input, request.input);
            assert_eq!(job.input_digest, digest);
            assert_eq!(job.intent_fingerprint, fingerprint);
            assert_eq!(job.load_executable_request()?, &request);
            assert_eq!(
                job.request
                    .grants
                    .subjektiv_journal
                    .as_ref()
                    .unwrap()
                    .binding_id,
                "source-lifetime"
            );
            assert_eq!(
                backend_job_recovery_store::history(conn, "w", &request.job_id)?,
                vec![before_attempt.clone()]
            );
            Ok(())
        };
        if version == 133 {
            migrate(&conn).unwrap();
            assert_eq!(current_schema_version(&conn).unwrap(), 134);
            assert_preserved(&conn).unwrap();
        }
        drop(conn);
        // The first managed open also exercises the full earlier classification
        // path for schema 129; the second proves reopening does not readmit input.
        for _ in 0..2 {
            let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
            store
                .with_conn(|conn| {
                    assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
                    assert_preserved(conn)
                })
                .unwrap();
            drop(store);
        }
    }

    #[test]
    fn finalization_migration_preserves_pending_journal_provenance_and_admission_bytes() {
        assert_saved_journal_request_survives_migration(133);
    }

    #[test]
    fn current_contract_migration_retains_journal_binding_and_original_executable_intent() {
        assert_saved_journal_request_survives_migration(129);
    }

    fn mixed_history_store() -> (tempfile::TempDir, SqliteWorkspaceStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.sqlite");
        super::super::tests::prepare_retained_schema(&path, 133);
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        seed(&conn);
        for (id, current_worker, prior_worker) in [
            (
                "release-success",
                "019a4ea8-0000-7000-8000-000000000003",
                "019a4ea8-0000-7000-8000-000000000004",
            ),
            (
                "release-failure",
                "019a4ea8-0000-7000-8000-000000000005",
                "019a4ea8-0000-7000-8000-000000000006",
            ),
        ] {
            // Retained cleanup is the historical fact, not a fabricated receipt.
            conn.execute(
                "UPDATE backend_jobs SET current_attempt=2 WHERE job_id=?1",
                [id],
            )
            .unwrap();
            conn.execute("UPDATE backend_job_attempts SET attempt=2,runtime_id='runtime',worker_id=?2,runtime_run_id=?1||'-new-run',worker_cleanup_state='pending' WHERE job_id=?1", params![id, current_worker]).unwrap();
            conn.execute("INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,input_digest,state,deadline_at,runtime_id,worker_id,runtime_run_id,worker_cleanup_state,worker_cleanup_completed_at,created_at,updated_at,completed_at) VALUES('w',?1,?1||'-prior',1,'digest','failed','deadline','old-runtime',?2,?1||'-old-run','completed','old-release','created','old-result','old-result')", params![id, prior_worker]).unwrap();
        }
        migrate(&conn).unwrap();
        drop(conn);
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        (dir, store)
    }

    fn finish_fixture_cleanup(
        store: &SqliteWorkspaceStore,
        attempt: &BackendJobAttemptRecord,
        owned: bool,
    ) -> Result<BackendJobAttemptRecord> {
        if owned {
            let history = store.list_backend_job_attempts("w", &attempt.job_id)?;
            store.finish_backend_job_worker_cleanup_owned(
                "w",
                &history,
                attempt,
                BackendJobWorkerCleanupState::Completed,
                None,
                None,
                "released",
            )
        } else {
            store.finish_backend_job_worker_cleanup(
                "w",
                &attempt.job_id,
                &attempt.attempt_id,
                BackendJobWorkerCleanupState::Completed,
                None,
                None,
                "released",
            )
        }
    }

    #[test]
    fn migrated_mixed_history_finalizes_only_after_new_cleanup_without_refencing_prior_release() {
        for owned in [false, true] {
            let (dir, store) = mixed_history_store();
            for (id, state) in [
                ("release-success", BackendJobState::Completed),
                ("release-failure", BackendJobState::Failed),
            ] {
                let before = store.get_backend_job("w", id).unwrap().unwrap();
                assert_eq!(before.state, BackendJobState::Finalizing);
                let prior = store
                    .get_backend_job_attempt("w", id, &format!("{id}-prior"))
                    .unwrap()
                    .unwrap();
                assert!(!store.has_backend_job_execution_fence("w", &prior).unwrap());
                // A completed cleanup replay must not revalidate historical receipts.
                assert_eq!(
                    finish_fixture_cleanup(&store, &prior, false).unwrap(),
                    prior
                );
                assert!(store.finalize_backend_job("w", id, "too-early").is_err());
                let (executing, acquired) = store
                    .claim_backend_job_worker_cleanup("w", id, id, "claim")
                    .unwrap();
                assert!(acquired);
                assert!(matches!(
                    finish_fixture_cleanup(&store, &executing, owned),
                    Err(Error::RepositoryConflict(_))
                ));
                assert_eq!(
                    store.get_backend_job_attempt("w", id, id).unwrap().unwrap(),
                    executing
                );
                assert_eq!(store.get_backend_job("w", id).unwrap().unwrap(), before);

                record_fixture_fence(&store, "w", &executing);
                assert!(
                    store
                        .finalize_backend_job("w", id, "still-too-early")
                        .is_err()
                );
                let released = finish_fixture_cleanup(&store, &executing, owned).unwrap();
                assert_eq!(
                    released.worker_cleanup_state,
                    Some(BackendJobWorkerCleanupState::Completed)
                );
                let completed = store.finalize_backend_job("w", id, "finalized").unwrap();
                assert_eq!(completed.state, state);
                assert_eq!(completed.result, before.result);
                assert_eq!(completed.completed_at.as_deref(), Some("finalized"));
                assert_eq!(
                    store
                        .get_backend_job_attempt("w", id, &prior.attempt_id)
                        .unwrap()
                        .unwrap(),
                    prior
                );
                assert!(!store.has_backend_job_execution_fence("w", &prior).unwrap());
            }
            drop(store);
            let store =
                SqliteWorkspaceStore::open_for_test(&dir.path().join("mixed.sqlite")).unwrap();
            for id in ["release-success", "release-failure"] {
                let saved = store.get_backend_job("w", id).unwrap().unwrap();
                assert_eq!(
                    store.finalize_backend_job("w", id, "replay").unwrap(),
                    saved
                );
            }
            store
                .with_conn(|conn| {
                    assert_eq!(
                        conn.query_row(
                            "SELECT count(*) FROM backend_job_execution_fences",
                            [],
                            |r| r.get::<_, i64>(0)
                        )?,
                        2
                    );
                    Ok(())
                })
                .unwrap();
        }
    }

    #[test]
    fn cleanup_completion_rejects_a_saved_fence_for_a_different_run() {
        for owned in [false, true] {
            let (_dir, store) = mixed_history_store();
            let id = "release-failure";
            let (executing, acquired) = store
                .claim_backend_job_worker_cleanup("w", id, id, "claim")
                .unwrap();
            assert!(acquired);
            record_fixture_fence(&store, "w", &executing);
            store.with_conn(|conn| {
                conn.execute("UPDATE backend_job_attempts SET runtime_run_id='different-run' WHERE job_id=?1 AND attempt=2", [id])?;
                Ok(())
            }).unwrap();
            let rebound = store.get_backend_job_attempt("w", id, id).unwrap().unwrap();
            assert!(
                !store
                    .has_backend_job_execution_fence("w", &rebound)
                    .unwrap()
            );
            assert!(matches!(
                finish_fixture_cleanup(&store, &rebound, owned),
                Err(Error::RepositoryConflict(_))
            ));
            assert_eq!(
                store.get_backend_job_attempt("w", id, id).unwrap().unwrap(),
                rebound
            );
            assert!(store.finalize_backend_job("w", id, "too-early").is_err());
        }
    }

    #[test]
    fn mixed_history_preallocation_rejection_finishes_without_inventing_execution_receipts() {
        for state in ["reserved", "dispatching"] {
            let (_dir, store) = mixed_history_store();
            let id = "release-failure";
            store.with_conn(|conn| {
                conn.execute("UPDATE backend_jobs SET state='pending' WHERE job_id=?1", [id])?;
                conn.execute("UPDATE backend_job_attempts SET state=?2,runtime_id=NULL,worker_id=NULL,runtime_run_id=NULL,worker_cleanup_state=NULL,completed_at=NULL WHERE job_id=?1 AND attempt=2", params![id,state])?;
                Ok(())
            }).unwrap();
            let failed = store
                .finish_backend_job_attempt(
                    "w",
                    id,
                    id,
                    BackendJobAttemptState::Failed,
                    "cancelled",
                    "No allocation receipt",
                    "rejected",
                )
                .unwrap();
            let precluded = state == "reserved";
            assert_eq!(failed.allocation_precluded_at.is_some(), precluded);
            assert_eq!(
                store.has_backend_job_execution_fence("w", &failed).unwrap(),
                precluded
            );
            assert_eq!(
                failed.worker_cleanup_state,
                Some(if precluded {
                    BackendJobWorkerCleanupState::Completed
                } else {
                    BackendJobWorkerCleanupState::Pending
                })
            );
            assert_eq!(
                store.get_backend_job("w", id).unwrap().unwrap().state,
                if precluded {
                    BackendJobState::Failed
                } else {
                    BackendJobState::Finalizing
                }
            );
            if !precluded {
                assert!(store.finalize_backend_job("w", id, "too-early").is_err());
            }
            store
                .with_conn(|conn| {
                    assert_eq!(
                        conn.query_row(
                            "SELECT count(*) FROM backend_job_execution_fences",
                            [],
                            |r| r.get::<_, i64>(0)
                        )?,
                        0
                    );
                    Ok(())
                })
                .unwrap();
        }
    }

    #[test]
    fn migration_failure_rolls_back_state_schema_and_receipt_before_same_boundary_retry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite");
        super::super::tests::prepare_retained_schema(&path, 133);
        let conn = Connection::open(&path).unwrap();
        configure_sqlite(&conn).unwrap();
        seed(&conn);
        // Abort after the table rebuild and state classification but before commit.
        conn.execute_batch("CREATE TEMP TRIGGER reject_completion_migration BEFORE INSERT ON main.__yoi_schema_migrations WHEN NEW.version=134 BEGIN SELECT RAISE(ABORT,'injected migration commit failure'); END;").unwrap();
        assert!(migrate(&conn).is_err());
        assert_eq!(current_schema_version(&conn).unwrap(), 133);
        storage_formats::verify_history(&conn, 133).unwrap();
        assert_eq!(
            read_backend_job(&conn, "w", "release-success")
                .unwrap()
                .unwrap()
                .state,
            BackendJobState::Completed
        );
        assert_eq!(
            conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, bool>(0))
                .unwrap(),
            true
        );
        conn.execute_batch("DROP TRIGGER reject_completion_migration;")
            .unwrap();
        migrate(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 134);
    }
}

/// Store fixtures substitute external receipts, not the finalization transaction.
#[cfg(test)]
pub(super) fn record_fixture_fence(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    attempt: &BackendJobAttemptRecord,
) {
    let host = crate::backend_job_host_effects::JobHostOwner::try_from(attempt).unwrap();
    let controller = serde_json::json!({
        "scope":"controller", "worker_ref":{"worker_id":attempt.worker.as_ref().unwrap().worker_id},
        "stop_operation_id":"019a4ea8-0000-7000-8000-000000000001",
        "execution_id":"019a4ea8-0000-7000-8000-000000000002",
        "shutdown_command_id":1,"submissions":[{"submission_request_id":attempt.runtime_run_id,"submission_id":"fixture-submission","disposition":"started"}],"feature_failures":[]
    });
    store
        .record_backend_job_execution_fence(
            workspace,
            attempt,
            &controller.to_string(),
            &serde_json::to_string(&host).unwrap(),
            "2026-11-04T00:00:00Z",
        )
        .unwrap();
}
