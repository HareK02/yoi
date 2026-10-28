use super::*;
use crate::tests::{candidate, legacy_retired_fixture, open_store, role};
use feature_storage::FeatureStorage;
use memory::extract::CandidateKind;

fn draft() -> MemoryDraft {
    MemoryDraft::active(
        CandidateKind::Lesson,
        "private claim",
        "private body",
        "useful",
        "initial",
    )
}

fn count(store: &SubjektivStore, table: &str) -> i64 {
    store
        .database
        .try_with_connection::<_, SubjektivError>(|c| {
            Ok(c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
        })
        .unwrap()
}

#[test]
fn fence_only_denies_content_reads_writes_and_exact_replay_before_purge() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store
        .create_subject_with_behavior(role(), "private behavior".into())
        .unwrap();
    let id = &subject.id;
    let attribution = SubjectSessionAttribution::new(id, "runtime", "worker", "session").unwrap();
    store
        .record_session_attribution(attribution.clone())
        .unwrap();
    let staged = candidate(id, "candidate", store.workspace_id());
    store.stage_candidate(staged.clone()).unwrap();
    let request = CandidateDecisionRequest {
        request_id: "decision".into(),
        candidate_id: "candidate".into(),
        reason: "adopt".into(),
        decision: CandidateDecision::Apply {
            target: MemoryChangeTarget::Create,
            draft: draft(),
        },
    };
    let memory = store
        .decide_candidate(id, request.clone())
        .unwrap()
        .memory
        .unwrap();
    let generation = store
        .prepare_job_surface_generation(id, Some(("job", "attempt")))
        .unwrap();
    let snapshot = store
        .publish_surface_generation(
            id,
            &generation.id,
            vec![SurfacePoint {
                body_md: "private surface".into(),
                memory_refs: vec![MemoryChangeRef {
                    memory_id: memory.id.clone(),
                    change_id: memory.change_id.clone(),
                }],
            }],
        )
        .unwrap();
    let fence = store.begin_subject_deletion(id, "delete").unwrap();
    assert!(!fence.completed);
    // The fence must work while every body remains physically present.
    assert_eq!(count(&store, "subjects"), 1);
    assert_eq!(count(&store, "memory_records"), 1);
    assert_eq!(count(&store, "candidate_decision_receipts"), 1);
    assert!(store.subject(id).unwrap().is_none());
    assert!(store.list_subjects(1).unwrap().items.is_empty());
    macro_rules! denied { ($($result:expr),+ $(,)?) => { $(assert!($result.is_err(), stringify!($result));)+ }; }
    denied!(
        store.session_attribution("session"),
        store.subject_session_attributions(id),
        store.subject_session_attribution_page(id, None, "2999", None, 1),
        store.record_session_attribution(attribution),
        store.staging_candidate(id, "candidate"),
        store.pending_staging_candidates(id, 1),
        store.pending_staging_backlog(id),
        store.staging_resolution(id, "candidate"),
        store.memory(id, &memory.id),
        store.memory_change(id, &memory.id, memory.change_id.clone()),
        store.scoped_memory(id, &memory.id),
        store.scoped_memory_change(id, &memory.id, memory.change_id.clone()),
        store.list_memories(id),
        store.list_memory_changes(id, &memory.id),
        store.memory_fingerprint(id),
        store.surface_snapshot(id, &snapshot.id),
        store.resident_surface(id),
        store.resident_context(id),
        store.require_job_surface_generation(id, &generation.id, "job", "attempt"),
        store.validate_job_surface_outcome(
            id,
            &generation.id,
            generation.memory_fingerprint,
            "ready",
            Some(&snapshot.id),
            None,
            "job",
            "attempt"
        ),
        store.prepare_surface_generation(id),
        store.publish_surface_generation(id, &generation.id, vec![]),
        store.fail_surface_generation(id, &generation.id, "failed"),
        store.update_subject_behavior(id, &subject.behavior_md, "late behavior".into()),
        store.stage_candidate(staged),
        store.decide_candidate(id, request),
        store.create_memory(id, draft()),
        store.subject_permissions(id),
        store.list_subject_worker_execution_grants(id),
    );
    assert_eq!(
        store.deletion_subject_workers(id).unwrap(),
        vec![("runtime".into(), "worker".into())]
    );
    assert_eq!(
        store.deletion_subject_sessions(id).unwrap(),
        vec!["session"]
    );
    assert_eq!(store.deletion_subject_jobs(id).unwrap()[0].job_id, "job");
    assert!(store.worker_subject_deleted("runtime", "worker").unwrap());
    assert!(
        store
            .finish_subject_deletion(id, "delete")
            .unwrap()
            .completed
    );
    for table in DIRECT {
        assert_eq!(count(&store, table), 0, "{table}");
    }
    assert!(store.worker_execution_grant("runtime", "worker").is_err());
    assert_eq!(
        store.deletion_subject_sessions(id).unwrap(),
        vec!["session"]
    );
    assert_eq!(
        store.finish_subject_deletion(id, "delete").unwrap(),
        store.begin_subject_deletion(id, "delete").unwrap()
    );
}

#[test]
fn purge_failure_rolls_back_content_but_not_the_committed_fence_and_restart_recovers() {
    let temp = tempfile::tempdir().unwrap();
    let id: String;
    let pending;
    {
        let (manager, scope, store) = open_store(temp.path(), "workspace-a");
        id = store.create_subject(role()).unwrap().id;
        store.create_memory(&id, draft()).unwrap();
        pending = store.begin_subject_deletion(&id, "delete").unwrap();
        store.database.try_with_connection::<_, SubjektivError>(|c| {
            c.execute_batch("CREATE TRIGGER injected_failure BEFORE DELETE ON subjects BEGIN SELECT RAISE(ABORT, 'simulated purge failure'); END;")?;
            Ok(())
        }).unwrap();
        assert!(store.finish_subject_deletion(&id, "delete").is_err());
        assert_eq!(count(&store, "memory_records"), 1);
        assert_eq!(count(&store, "memory_changes"), 1);
        assert_eq!(store.subject_deletion(&id).unwrap(), Some(pending.clone()));
        drop(store);
        drop(scope);
        manager.shutdown().unwrap();
    }
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    assert_eq!(
        store.pending_subject_deletions().unwrap(),
        vec![pending.clone()]
    );
    assert_eq!(
        store.begin_subject_deletion(&id, "delete").unwrap(),
        pending
    );
    assert!(store.list_memories(&id).is_err());
    store
        .database
        .try_with_connection::<_, SubjektivError>(|c| {
            c.execute_batch("DROP TRIGGER injected_failure")?;
            Ok(())
        })
        .unwrap();
    assert!(
        store
            .finish_subject_deletion(&id, "delete")
            .unwrap()
            .completed
    );
    assert!(store.pending_subject_deletions().unwrap().is_empty());
}

#[test]
fn deletion_identity_conflicts_are_scope_wide_but_do_not_cross_workspaces() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let a = store.create_subject(role()).unwrap().id;
    let b = store.create_subject(role()).unwrap().id;
    let saved = store.begin_subject_deletion(&a, "same-key").unwrap();
    assert_eq!(store.begin_subject_deletion(&a, "same-key").unwrap(), saved);
    for result in [
        store.begin_subject_deletion(&a, "different"),
        store.begin_subject_deletion(&b, "same-key"),
        store.finish_subject_deletion(&a, "different"),
    ] {
        assert!(matches!(
            result,
            Err(SubjektivError::SubjectDeletionConflict(_))
        ));
    }
    assert!(store.begin_subject_deletion("absent", "new-key").is_err());
    assert!(store.finish_subject_deletion(&b, "same-key").is_err());
    let page = store.list_subjects(1).unwrap();
    assert_eq!(page.items[0].id, b);
    assert!(!page.has_more);
    store.finish_subject_deletion(&a, "same-key").unwrap();
    store.create_memory(&b, draft()).unwrap();
    let (_other_manager, _other_scope, other) = open_store(temp.path(), "workspace-b");
    assert!(other.subject_deletion(&a).unwrap().is_none());
    assert!(other.begin_subject_deletion(&b, "same-key").is_err());
    let c = other.create_subject(role()).unwrap().id;
    other.begin_subject_deletion(&c, "same-key").unwrap();
    assert_eq!(store.list_memories(&b).unwrap().len(), 1);
    let fresh = store.create_subject(role()).unwrap().id;
    assert!(store.list_memories(&fresh).unwrap().is_empty());
    assert!(store.subject_permissions(&fresh).unwrap().is_none());
}

#[test]
fn concurrent_begin_and_write_serialize_and_exact_finish_is_replayable() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let id = store.create_subject(role()).unwrap().id;
    let barrier = std::sync::Barrier::new(3);
    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            store.begin_subject_deletion(&id, "delete")
        });
        let b = scope.spawn(|| {
            barrier.wait();
            store.create_memory(&id, draft())
        });
        barrier.wait();
        assert!(!a.join().unwrap().unwrap().completed);
        // A write either precedes begin and is later purged, or loses to the fence.
        let _ = b.join().unwrap();
    });
    assert!(store.create_memory(&id, draft()).is_err());
    assert_eq!(store.pending_subject_deletions().unwrap().len(), 1);
    store.finish_subject_deletion(&id, "delete").unwrap();
    assert_eq!(count(&store, "memory_records"), 0);
}

#[test]
fn every_subject_owned_table_has_old_and_new_sql_fences_and_retention_is_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let a = store.create_subject(role()).unwrap().id;
    let b = store.create_subject(role()).unwrap().id;
    store
        .stage_candidate(candidate(&a, "a", store.workspace_id()))
        .unwrap();
    store
        .stage_candidate(candidate(&b, "b", store.workspace_id()))
        .unwrap();
    store.begin_subject_deletion(&a, "delete").unwrap();
    store.database.try_with_connection::<_, SubjektivError>(|c| {
        let tables = c.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")?.query_map([], |r| r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
        for table in tables {
            let columns = c.prepare(&format!("PRAGMA table_info({table})"))?.query_map([], |r| r.get::<_,String>(1))?.collect::<std::result::Result<Vec<_>,_>>()?;
            if !columns.iter().any(|v| v == "subject_id") || table.starts_with("subject_delet") { continue; }
            assert!(DIRECT.contains(&table.as_str()) || table == "journal_erased_sessions", "unclassified Subject table: {table}");
        }
        for table in DIRECT.iter().copied().chain(INDIRECT.iter().map(|v| v.0)).chain(["journal_erased_sessions"]) {
            let sql: String = c.query_row("SELECT sql FROM sqlite_master WHERE name=?1", [format!("deletion_fence_{table}_UPDATE")], |r| r.get(0))?;
            assert!(sql.contains("OLD.") && sql.contains("NEW."), "{table}: {sql}");
        }
        // OLD fenced -> NEW unfenced must not evade a guard.
        assert!(c.execute("UPDATE subjects SET subject_id=?2 WHERE subject_id=?1", params![a, "new-id"]).is_err());
        assert!(c.execute("UPDATE subjects SET subject_id=?2 WHERE subject_id=?1", params![b, a]).is_err());
        assert!(c.execute("DELETE FROM staging_records WHERE subject_id=?1", [&b]).is_err());
        assert!(c.execute("DELETE FROM subject_deletions WHERE subject_id=?1", [&a]).is_err());
        assert!(c.execute("UPDATE subject_deletions SET operation_id='changed' WHERE subject_id=?1", [&a]).is_err());
        Ok(())
    }).unwrap();
    store.finish_subject_deletion(&a, "delete").unwrap();
    assert!(store.staging_candidate(&b, "b").unwrap().is_some());
}

#[test]
fn legacy_retired_is_readable_serde_compatible_and_explicitly_deletable() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let id = store.create_subject(role()).unwrap().id;
    store.create_memory(&id, draft()).unwrap();
    legacy_retired_fixture(&store, &id).unwrap();
    assert_eq!(
        serde_json::from_str::<SubjectState>("\"retired\"").unwrap(),
        SubjectState::LegacyRetired
    );
    assert_eq!(
        serde_json::to_string(&SubjectState::LegacyRetired).unwrap(),
        "\"retired\""
    );
    assert_eq!(store.list_memories(&id).unwrap().len(), 1);
    assert!(store.create_memory(&id, draft()).is_err());
    store.begin_subject_deletion(&id, "delete-legacy").unwrap();
    store.finish_subject_deletion(&id, "delete-legacy").unwrap();
    assert_eq!(count(&store, "memory_changes"), 0);
}

#[test]
fn migration_preserves_legacy_content_until_explicit_delete_including_archive() {
    let temp = tempfile::tempdir().unwrap();
    let id: String;
    {
        let manager = FeatureStorage::new(temp.path());
        let scope = manager.workspace("workspace-a").unwrap();
        let registration = scope
            .register(FeatureRegistration::new(
                SUBJEKTIV_FEATURE_ID,
                &MIGRATIONS[..6],
            ))
            .unwrap();
        let store = SubjektivStore::open(&scope, &registration).unwrap();
        id = "legacy-subject".into();
        store.database.try_transaction::<_, SubjektivError>(|tx| {
            let raw = serde_json::json!({"schema_version":1,"id":id,"role":"workspace_companion","behavior_md":"legacy private behavior","state":"active","created_at":"2026-01-01","updated_at":"2026-01-01"}).to_string();
            tx.execute("INSERT INTO subjects(subject_id,role,state,store_revision,record_json,created_at,updated_at) VALUES(?1,'workspace_companion','active',0,?2,'2026-01-01','2026-01-01')", params![id,raw])?;
            Ok(())
        }).unwrap();
        // Build a saved SQL fixture rather than using current-only APIs against an old schema.
        store
            .database
            .try_transaction::<_, SubjektivError>(|tx| {
                let raw: String = tx.query_row(
                    "SELECT record_json FROM subjects WHERE subject_id=?1",
                    [&id],
                    |r| r.get(0),
                )?;
                let mut value: serde_json::Value = serde_json::from_str(&raw)?;
                value["state"] = "retired".into();
                tx.execute(
                    "UPDATE subjects SET state='retired',record_json=?2 WHERE subject_id=?1",
                    params![id, value.to_string()],
                )?;
                Ok(())
            })
            .unwrap();
        drop(store);
        drop(scope);
        manager.shutdown().unwrap();
    }
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    assert_eq!(
        store.subject(&id).unwrap().unwrap().state,
        SubjectState::LegacyRetired
    );
    assert!(count(&store, "legacy_record_archive") > 1);
    store.begin_subject_deletion(&id, "delete").unwrap();
    store.finish_subject_deletion(&id, "delete").unwrap();
    store.database.try_with_connection::<_, SubjektivError>(|c| {
        let remaining: i64 = c.query_row("SELECT COUNT(*) FROM legacy_record_archive WHERE json_extract(row_json,'$.subject_id')=?1", [&id], |r|r.get(0))?;
        assert_eq!(remaining, 0);
        // Workspace-only metadata remains and is not a copy of the Subject's content.
        assert_eq!(c.query_row("SELECT COUNT(*) FROM legacy_record_archive WHERE source_table='store_scope'", [], |r|r.get::<_,i64>(0))?, 1);
        Ok(())
    }).unwrap();
}

#[test]
fn concurrent_deletions_on_independent_connections_save_one_request_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let (_second_manager, _second_scope, second) = open_store(temp.path(), "workspace-a");
    let id = store.create_subject(role()).unwrap().id;
    let barrier = std::sync::Barrier::new(3);
    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            store.begin_subject_deletion(&id, "first")
        });
        let b = scope.spawn(|| {
            barrier.wait();
            second.begin_subject_deletion(&id, "second")
        });
        barrier.wait();
        let results = [a.join().unwrap(), b.join().unwrap()];
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(SubjektivError::SubjectDeletionConflict(_))))
                .count(),
            1
        );
    });
    let saved = second.subject_deletion(&id).unwrap().unwrap();
    assert_eq!(
        store
            .begin_subject_deletion(&id, &saved.operation_id)
            .unwrap(),
        saved
    );
    std::thread::scope(|scope| {
        let a = scope.spawn(|| store.finish_subject_deletion(&id, &saved.operation_id));
        let b = scope.spawn(|| second.finish_subject_deletion(&id, &saved.operation_id));
        assert_eq!(a.join().unwrap().unwrap(), b.join().unwrap().unwrap());
    });
}

#[test]
fn deletion_migration_failure_rolls_back_fences_and_trigger_rewrites_then_retries() {
    let temp = tempfile::tempdir().unwrap();
    let id;
    {
        let manager = FeatureStorage::new(temp.path());
        let scope = manager.workspace("workspace-a").unwrap();
        let registration = scope
            .register(FeatureRegistration::new(
                SUBJEKTIV_FEATURE_ID,
                &MIGRATIONS[..13],
            ))
            .unwrap();
        let store = SubjektivStore::open(&scope, &registration).unwrap();
        id = store.create_subject(role()).unwrap().id;
        store
            .stage_candidate(candidate(&id, "preserved", store.workspace_id()))
            .unwrap();
        store.database.try_with_connection::<_, SubjektivError>(|c| {
            // Fail near the end, after retention triggers have already been rewritten.
            c.execute_batch("CREATE TRIGGER deletion_fence_journal_sources_INSERT BEFORE INSERT ON journal_sources BEGIN SELECT 1; END;")?;
            Ok(())
        }).unwrap();
        drop(store);
        drop(scope);
        manager.shutdown().unwrap();
    }
    let manager = FeatureStorage::new(temp.path());
    let scope = manager.workspace("workspace-a").unwrap();
    let current = SubjektivStore::register(&scope).unwrap();
    assert!(SubjektivStore::open(&scope, &current).is_err());
    drop(scope);
    manager.shutdown().unwrap();
    let manager = FeatureStorage::new(temp.path());
    let scope = manager.workspace("workspace-a").unwrap();
    // Reopen only the fixture's pre-deletion registration to inspect the failed migration.
    let old = scope
        .register(FeatureRegistration::new(
            SUBJEKTIV_FEATURE_ID,
            &MIGRATIONS[..13],
        ))
        .unwrap();
    let store = SubjektivStore::open(&scope, &old).unwrap();
    store
        .database
        .try_with_connection::<_, SubjektivError>(|c| {
            assert!(!c.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='subject_deletions')",
                [],
                |r| r.get::<_, bool>(0)
            )?);
            let sql: String = c.query_row(
                "SELECT sql FROM sqlite_master WHERE name='staging_records_no_delete'",
                [],
                |r| r.get(0),
            )?;
            assert!(!sql.contains("subject_deletions"));
            assert!(
                c.execute("DELETE FROM staging_records WHERE subject_id=?1", [&id])
                    .is_err()
            );
            c.execute_batch("DROP TRIGGER deletion_fence_journal_sources_INSERT;")?;
            Ok(())
        })
        .unwrap();
    drop(store);
    drop(scope);
    manager.shutdown().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    assert!(store.staging_candidate(&id, "preserved").unwrap().is_some());
    assert!(store.pending_subject_deletions().unwrap().is_empty());
    store.begin_subject_deletion(&id, "delete").unwrap();
    store.finish_subject_deletion(&id, "delete").unwrap();
}
