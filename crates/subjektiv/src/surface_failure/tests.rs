use super::*;

struct Fixture {
    store: SubjektivStore,
    _scope: ScopedFeatureStorage,
    _storage: feature_storage::FeatureStorage,
}
impl std::ops::Deref for Fixture {
    type Target = SubjektivStore;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

fn open(root: &std::path::Path, registration: FeatureRegistration) -> Fixture {
    let storage = feature_storage::FeatureStorage::new(root);
    let scope = storage.scope("surface-failure-tests").unwrap();
    let registered = scope.register(registration).unwrap();
    Fixture {
        store: SubjektivStore::from_database(scope.open(&registered).unwrap()).unwrap(),
        _scope: scope,
        _storage: storage,
    }
}

#[test]
fn failure_receipt_retains_bounded_detail_and_rejects_rewriting() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path(), REGISTRATION);
    let subject = store
        .create_subject(SubjectRole::new("companion").unwrap())
        .unwrap();
    let run = store
        .prepare_job_surface_generation(&subject.id, Some(("job", "attempt")))
        .unwrap();
    let detail = "Internal: surface editor finished without valid structured output";
    store
        .fail_surface_generation_with_detail(&subject.id, &run.id, "editor_failed", Some(detail))
        .unwrap();
    store
        .fail_surface_generation_with_detail(&subject.id, &run.id, "editor_failed", Some(detail))
        .unwrap();
    assert!(
        store
            .fail_surface_generation_with_detail(
                &subject.id,
                &run.id,
                "editor_failed",
                Some("different")
            )
            .is_err()
    );
    let saved = store.memory_connection(|c| Ok(c.query_row(
        "SELECT reason_code,detail FROM surface_generation_failures WHERE subject_id=?1 AND generation_id=?2",
        params![subject.id,run.id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)),
    )?)).unwrap();
    assert_eq!(saved, ("editor_failed".into(), detail.into()));
    let other = store
        .create_subject(SubjectRole::new("other").unwrap())
        .unwrap();
    assert!(
        store
            .fail_surface_generation_with_detail(&other.id, &run.id, "editor_failed", Some(detail))
            .is_err()
    );
    let fresh = store.prepare_surface_generation(&subject.id).unwrap();
    assert!(
        store
            .fail_surface_generation_with_detail(
                &subject.id,
                &fresh.id,
                "editor_failed",
                Some(&"x".repeat(server_api::SUBJEKTIV_SURFACE_FAILURE_DETAIL_MAX_BYTES + 1))
            )
            .is_err()
    );
    assert!(
        store
            .fail_surface_generation_with_detail(
                &subject.id,
                &fresh.id,
                "editor_failed",
                Some("bad\0detail")
            )
            .is_err()
    );
    let result = crate::job::lookup_result(&store, &subject.id, &[], "job", "attempt")
        .unwrap()
        .unwrap();
    assert!(
        !result.to_string().contains(detail),
        "diagnostics must not become the resident surface or Job result"
    );
    assert!(
        store
            .resident_surface(&subject.id)
            .unwrap()
            .snapshot
            .is_none()
    );
    drop(store);
    let store = open(temp.path(), REGISTRATION);
    store.memory_connection(|c| {
        assert_eq!(c.query_row("SELECT detail FROM surface_generation_failures WHERE subject_id=?1 AND generation_id=?2",params![subject.id,run.id],|r|r.get::<_,String>(0))?,detail);
        assert!(c.execute("UPDATE surface_generation_failures SET detail='rewrite'",[]).is_err());
        assert!(c.execute("DELETE FROM surface_generation_failures",[]).is_err());
        Ok(())
    }).unwrap();
    store.begin_subject_deletion(&subject.id, "delete").unwrap();
    assert!(
        store
            .fail_surface_generation(&subject.id, &fresh.id, "late")
            .is_err()
    );
    store
        .finish_subject_deletion(&subject.id, "delete")
        .unwrap();
    store
        .memory_connection(|c| {
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM surface_generation_failures",
                    [],
                    |r| r.get::<_, u64>(0)
                )?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn migration_does_not_assign_legacy_failure_to_a_prepared_attempt() {
    let temp = tempfile::tempdir().unwrap();
    let (subject, run);
    {
        let store = open(
            temp.path(),
            FeatureRegistration::new(SUBJEKTIV_FEATURE_ID, &MIGRATIONS[..17]),
        );
        subject = store
            .create_subject(SubjectRole::new("legacy").unwrap())
            .unwrap();
        run = store
            .prepare_job_surface_generation(&subject.id, Some(("job", "attempt")))
            .unwrap();
        store.memory_transaction(|tx| {
            tx.execute("INSERT INTO surface_generation_state(subject_id,memory_fingerprint,status,reason_code,updated_at) VALUES(?1,?2,'failed','editor_failed','original-time')",
                params![subject.id,run.memory_fingerprint])?;
            Ok(())
        }).unwrap();
    }
    let store = open(temp.path(), REGISTRATION);
    assert_eq!(
        store.resident_surface(&subject.id).unwrap().availability,
        SurfaceAvailability::Failed
    );
    assert!(
        crate::job::lookup_result(&store, &subject.id, &[], "job", "attempt")
            .unwrap()
            .is_none()
    );
    store
        .memory_connection(|c| {
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM surface_generation_failures",
                    [],
                    |r| r.get::<_, u64>(0)
                )?,
                0
            );
            assert_eq!(
                c.query_row(
                    "SELECT updated_at FROM surface_generation_state WHERE subject_id=?1",
                    [&subject.id],
                    |r| r.get::<_, String>(0)
                )?,
                "original-time"
            );
            Ok(())
        })
        .unwrap();
    drop(store);
    let reopened = open(temp.path(), REGISTRATION);
    assert!(
        crate::job::lookup_result(&reopened, &subject.id, &[], "job", "attempt")
            .unwrap()
            .is_none()
    );
}

#[test]
fn failure_receipt_and_projection_roll_back_together() {
    let temp = tempfile::tempdir().unwrap();
    let store = open(temp.path(), REGISTRATION);
    let subject = store
        .create_subject(SubjectRole::new("atomic").unwrap())
        .unwrap();
    let run = store.prepare_surface_generation(&subject.id).unwrap();
    store.memory_connection(|c| {
        c.execute_batch("CREATE TRIGGER reject_failure_projection BEFORE INSERT ON surface_generation_state BEGIN SELECT RAISE(ABORT, 'injected storage failure'); END;")?;
        Ok(())
    }).unwrap();
    assert!(
        store
            .fail_surface_generation_with_detail(
                &subject.id,
                &run.id,
                "editor_failed",
                Some("bounded diagnostic")
            )
            .is_err()
    );
    store
        .memory_connection(|c| {
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM surface_generation_failures",
                    [],
                    |r| r.get::<_, u64>(0)
                )?,
                0
            );
            c.execute_batch("DROP TRIGGER reject_failure_projection")?;
            Ok(())
        })
        .unwrap();
    store
        .fail_surface_generation_with_detail(
            &subject.id,
            &run.id,
            "editor_failed",
            Some("bounded diagnostic"),
        )
        .unwrap();
    assert_eq!(
        store.resident_surface(&subject.id).unwrap().availability,
        SurfaceAvailability::Failed
    );
}
