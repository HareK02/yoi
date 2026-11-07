//! Real local Subject lease, Job schema migration and Feature database retirement.
//! Only execution is omitted: all Journal artifacts are produced by the real adapter.
use super::*;
use crate::job_store::{JobStore, JobStoreError};
use rusqlite::{Connection, params, types::Value};

fn domain(fixture: &Fixture) -> Connection {
    Connection::open(
        fixture
            .host()
            .catalog
            .root
            .join("features")
            .join(&fixture.host().catalog.scope_id)
            .join("features/subjektiv.sqlite"),
    )
    .unwrap()
}

fn rows(conn: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut statement = conn.prepare(sql).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            (0..columns)
                .map(|i| row.get(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn artifacts(conn: &Connection) -> Vec<Vec<Vec<Value>>> {
    let mut saved = Vec::new();
    for table in [
        "journals",
        "journal_entries",
        "journal_corrections",
        "journal_sources",
        "journal_operations",
        "journal_writer_bindings",
        "journal_session_bindings",
    ] {
        saved.push(rows(
            conn,
            &format!("SELECT rowid,* FROM {table} ORDER BY rowid"),
        ));
    }
    saved.push(rows(conn, "SELECT rowid,id,journal_id,job_id,attempt_id,request_json,snapshot_json,completion_json FROM journal_generations ORDER BY rowid"));
    saved
}

fn active(conn: &Connection, generation: &str) -> bool {
    conn.query_row(
        "SELECT active FROM journal_generations WHERE id=?1",
        [generation],
        |r| r.get(0),
    )
    .unwrap()
}

fn queued(path: &std::path::Path) -> Vec<Vec<Value>> {
    rows(
        &Connection::open(path).unwrap(),
        "SELECT job_id,domain_references FROM job_reference_retirements ORDER BY job_id",
    )
}

fn reconnect(fixture: &Fixture) -> Result<SubjectConnection, SubjectError> {
    StandaloneSubjects::open_existing(&fixture.host().catalog.state_root)?.connect(
        &fixture.connection.binding.subject_id,
        fixture.host().worker_id,
    )
}

fn bind_jobs(fixture: &Fixture) {
    let jobs = crate::jobs::StandaloneJobs::open(
        &fixture.connection.jobs_path,
        fixture._temp.path().into(),
    )
    .unwrap();
    fixture.host().bind_jobs(jobs).unwrap();
}

/// Turn the real saved Journal grant into a pre-current contract, not a synthetic
/// queue row. Remove the new migration metadata to model a schema-9 database.
fn downgrade(fixture: &Fixture, job: &str) {
    let conn = Connection::open(&fixture.connection.jobs_path).unwrap();
    conn.execute_batch("DROP TRIGGER job_grant_immutable;")
        .unwrap();
    conn.execute("UPDATE job_domain_grants SET grant_json=json_remove(grant_json,'$.writer_binding_id') WHERE job_id=?1", [job]).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER job_grant_immutable BEFORE UPDATE ON job_domain_grants
        BEGIN SELECT RAISE(ABORT, 'domain grants are immutable'); END;
        DROP TABLE job_reference_retirements;
        DROP TABLE job_reference_retirement_target;
        PRAGMA user_version=9;",
    )
    .unwrap();
}

fn saved_fixture(completed: bool) -> (Fixture, String, String) {
    let fixture = Fixture::new("immutable old Journal source");
    let (adapter, _, job) = fixture.adapter();
    let initial = adapter.state().unwrap();
    let receipt = save(
        &adapter,
        "saved-record",
        &initial.current_journal.state_token,
        JournalAction::Create {
            title: "retained title".into(),
            body: "retained body".into(),
            ongoing: "unfinished".into(),
            state: JournalEntryState::Open,
            source_end: None,
            correction_refs: vec![],
        },
        initial.selected_ranges.clone(),
    );
    if completed {
        let position = save(
            &adapter,
            "saved-position",
            &receipt.state_token,
            JournalAction::UpdatePosition {
                current_position: "recorded".into(),
            },
            vec![],
        );
        save(
            &adapter,
            "saved-checkpoint",
            &position.state_token,
            JournalAction::Checkpoint {
                record_operation_ids: vec!["saved-record".into()],
            },
            initial.selected_ranges,
        );
        adapter.finish().unwrap();
    }
    let generation = adapter.generation.id.clone();
    drop(adapter);
    (fixture, job.request.job_id, generation)
}

#[test]
fn migration_retires_only_selected_subject_generations_preserves_artifacts_and_allows_new_activity()
{
    for completed in [false, true] {
        let (mut fixture, job, generation) = saved_fixture(completed);
        let other = fixture
            .host()
            .catalog
            .create("other subject", None)
            .unwrap();
        let db = domain(&fixture);
        // Shared physical Feature DB: the same Job ID in another Subject and an
        // unrelated Job in this Subject must both retain their active generations.
        for (id, subject, job_id) in [
            ("other", &other.id, &job),
            (
                "survivor",
                &fixture.connection.binding.subject_id,
                &"unrelated-job".to_owned(),
            ),
        ] {
            db.execute("INSERT INTO journals(id,subject_id,source_session_id,title,current_position,state_token,created_at,updated_at) VALUES(?1,?2,?1,'untouched','untouched','untouched','1','1')", params![id,subject]).unwrap();
            db.execute("INSERT INTO journal_generations(id,journal_id,job_id,attempt_id,request_json,snapshot_json,active) SELECT ?1,?1,?2,attempt_id,request_json,snapshot_json,1 FROM journal_generations WHERE id=?3", params![id,job_id,generation]).unwrap();
        }
        let before = artifacts(&db);
        fixture.connection.close().unwrap();
        downgrade(&fixture, &job);
        fixture.connection = reconnect(&fixture).unwrap();
        assert!(!active(&db, &generation));
        assert!(active(&db, "other"));
        assert!(active(&db, "survivor"));
        assert_eq!(artifacts(&db), before);
        assert!(queued(&fixture.connection.jobs_path).is_empty());
        bind_jobs(&fixture);
        assert!(matches!(
            fixture.host().jobs().unwrap().get(&job),
            Err(crate::jobs::JobServiceError::Store(
                JobStoreError::NotFound(_)
            ))
        ));

        // Idempotent current-schema reopen must not erase receipts or other work.
        fixture.connection.close().unwrap();
        fixture.connection = reconnect(&fixture).unwrap();
        assert_eq!(artifacts(&db), before);
        bind_jobs(&fixture);
        fixture.append("new", "new activity after migration");
        let (adapter, _, new_job) = fixture.adapter();
        assert_ne!(new_job.request.job_id, job);
        assert_ne!(adapter.generation.id, generation);
        assert!(active(&db, &adapter.generation.id));
        assert!(
            new_job
                .request
                .input
                .to_string()
                .contains("new activity after migration")
        );
        let state = adapter.state().unwrap();
        save(
            &adapter,
            "new-position",
            &state.current_journal.state_token,
            JournalAction::UpdatePosition {
                current_position: "new generation works".into(),
            },
            vec![],
        );
    }
}

#[test]
fn feature_failure_retains_exact_queue_and_pin_then_retries_without_publishing_host() {
    let (mut fixture, job, generation) = saved_fixture(false);
    let db = domain(&fixture);
    let before = artifacts(&db);
    fixture.connection.close().unwrap();
    downgrade(&fixture, &job);
    let catalog = StandaloneSubjects::open_existing(&fixture.host().catalog.state_root).unwrap();
    // Inject after FeatureStorage has validated the on-disk schema, so this
    // failure exercises the retirement transaction rather than schema admission.
    db.execute_batch(
        "CREATE TRIGGER refuse_retirement BEFORE UPDATE OF active ON journal_generations
        BEGIN SELECT RAISE(ABORT,'injected Feature retirement failure'); END;",
    )
    .unwrap();
    match catalog.connect(
        &fixture.connection.binding.subject_id,
        fixture.host().worker_id,
    ) {
        Err(SubjectError::Domain(error)) => assert!(
            error
                .to_string()
                .contains("injected Feature retirement failure"),
            "{error}"
        ),
        Err(error) => panic!("unexpected failure: {error}"),
        Ok(_) => panic!("retirement should fail closed"),
    }
    assert!(active(&db, &generation));
    assert_eq!(artifacts(&db), before);
    assert_eq!(
        queued(&fixture.connection.jobs_path),
        vec![vec![Value::Text(job), Value::Integer(1)]]
    );
    let conn = Connection::open(&fixture.connection.jobs_path).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        10
    );
    assert_eq!(
        rows(&conn, "SELECT * FROM job_reference_retirement_target").len(),
        1
    );
    // The production generic Host entry cannot run restart reconciliation first.
    assert!(matches!(
        crate::jobs::StandaloneJobs::open_for_host(
            &fixture.connection.jobs_path,
            fixture._temp.path().into(),
            fixture._allocation.as_ref().unwrap().lease()
        ),
        Err(crate::jobs::JobServiceError::Store(
            JobStoreError::InvalidTransition(
                "pending Job reference retirement requires the owning Subject Host"
            )
        ))
    ));
    db.execute_batch("DROP TRIGGER refuse_retirement;").unwrap();
    let physical: String = conn
        .query_row(
            "SELECT physical_id FROM job_reference_retirement_target",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute_batch(
        "UPDATE job_reference_retirement_target SET physical_id='different-physical-database';",
    )
    .unwrap();
    assert!(matches!(
        reconnect(&fixture),
        Err(SubjectError::JobMigration(
            JobStoreError::InvalidTransition("Job reference retirement target changed")
        ))
    ));
    assert!(
        active(&db, &generation),
        "a pending target mismatch must precede Feature effects"
    );
    assert_eq!(queued(&fixture.connection.jobs_path).len(), 1);
    conn.execute(
        "UPDATE job_reference_retirement_target SET physical_id=?1",
        [&physical],
    )
    .unwrap();
    fixture.connection = reconnect(&fixture).unwrap();
    assert!(!active(&db, &generation));
    assert!(queued(&fixture.connection.jobs_path).is_empty());
    assert_eq!(artifacts(&db), before);
}

#[test]
fn acknowledgement_failure_replays_committed_feature_retirement_on_reopen() {
    let (mut fixture, job, generation) = saved_fixture(false);
    let db = domain(&fixture);
    let before = artifacts(&db);
    fixture.connection.close().unwrap();
    downgrade(&fixture, &job);
    drop(JobStore::open(&fixture.connection.jobs_path).unwrap());
    let conn = Connection::open(&fixture.connection.jobs_path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER refuse_ack BEFORE DELETE ON job_reference_retirements
        BEGIN SELECT RAISE(ABORT,'injected queue acknowledgement failure'); END;",
    )
    .unwrap();
    assert!(matches!(
        reconnect(&fixture),
        Err(SubjectError::JobMigration(_))
    ));
    assert!(
        !active(&db, &generation),
        "Feature commit precedes queue acknowledgement"
    );
    assert_eq!(
        queued(&fixture.connection.jobs_path),
        vec![vec![Value::Text(job), Value::Integer(1)]]
    );
    assert_eq!(artifacts(&db), before);
    conn.execute_batch("DROP TRIGGER refuse_ack;").unwrap();
    fixture.connection = reconnect(&fixture).unwrap();
    assert!(queued(&fixture.connection.jobs_path).is_empty());
    assert_eq!(artifacts(&db), before);
}

#[test]
fn generic_host_finishes_only_unpinned_domainless_migration_before_recovery() {
    let fixture = Fixture::new("request template");
    let (_, _, template) = fixture.adapter();
    for pinned in [false, true] {
        let path = fixture
            ._temp
            .path()
            .join(format!("generic-{pinned}.sqlite3"));
        let mut store = JobStore::open(&path).unwrap();
        let mut request = template.request.clone();
        request.job_id = "obsolete-generic".into();
        request.purpose = "generic-test".into();
        request.serialization_key = None;
        store.reserve(request.clone()).unwrap();
        request.job_id = "current-generic".into();
        let current = store.reserve(request).unwrap();
        let dispatched = store
            .dispatch("current-generic", &current.attempt.attempt_id, 8)
            .unwrap();
        drop(store);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("DROP TRIGGER job_input_retirement;
            UPDATE job_intents SET request_json=json_set(request_json,'$.input_revision','obsolete') WHERE job_id='obsolete-generic';
            PRAGMA user_version=9;").unwrap();
        let mut migrated = JobStore::open(&path).unwrap();
        assert_eq!(
            queued(&path),
            vec![vec![
                Value::Text("obsolete-generic".into()),
                Value::Integer(0)
            ]]
        );
        if pinned {
            migrated
                .pin_reference_retirement(
                    "pinned-subject",
                    &feature_storage::DatabaseIdentity {
                        scope: "scope".into(),
                        feature: "subjektiv".into(),
                        physical: "physical".into(),
                    },
                )
                .unwrap();
        }
        drop(migrated);
        let opened = crate::jobs::StandaloneJobs::open_for_host(
            &path,
            fixture._temp.path().into(),
            fixture._allocation.as_ref().unwrap().lease(),
        );
        if pinned {
            assert!(matches!(
                opened,
                Err(crate::jobs::JobServiceError::Store(
                    JobStoreError::InvalidTransition(
                        "pending Job reference retirement requires the owning Subject Host"
                    )
                ))
            ));
            assert_eq!(
                JobStore::open(&path)
                    .unwrap()
                    .get("current-generic")
                    .unwrap(),
                dispatched,
                "failed-closed open must not reconcile a surviving execution"
            );
            assert_eq!(queued(&path).len(), 1);
        } else {
            let jobs = opened.unwrap();
            assert!(queued(&path).is_empty());
            let recovered = jobs.get("current-generic").unwrap();
            assert_eq!(recovered.state, JobState::Failed);
            assert_eq!(recovered.request, dispatched.request);
            assert_eq!(recovered.executions, dispatched.executions);
            assert!(matches!(
                jobs.get("obsolete-generic"),
                Err(crate::jobs::JobServiceError::Store(
                    JobStoreError::NotFound(_)
                ))
            ));
        }
    }
}

#[test]
fn lease_and_pinned_target_fence_retirement_even_after_queue_is_drained() {
    let (mut fixture, job, generation) = saved_fixture(false);
    let db = domain(&fixture);
    // A competing connection cannot even perform Job schema migration.
    downgrade(&fixture, &job);
    assert!(matches!(reconnect(&fixture), Err(SubjectError::Active(_))));
    assert!(active(&db, &generation));
    let conn = Connection::open(&fixture.connection.jobs_path).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        9
    );
    fixture.connection.close().unwrap();
    fixture.connection = reconnect(&fixture).unwrap();
    fixture.connection.close().unwrap();
    assert!(queued(&fixture.connection.jobs_path).is_empty());
    let before = artifacts(&db);
    for field in ["scope_id", "feature_id", "physical_id", "subject_id"] {
        let original: String = conn
            .query_row(
                &format!("SELECT {field} FROM job_reference_retirement_target"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            &format!("UPDATE job_reference_retirement_target SET {field}='different-target'"),
            [],
        )
        .unwrap();
        assert!(
            matches!(
                reconnect(&fixture),
                Err(SubjectError::JobMigration(
                    JobStoreError::InvalidTransition("Job reference retirement target changed")
                ))
            ),
            "{field}"
        );
        assert_eq!(artifacts(&db), before);
        conn.execute(
            &format!("UPDATE job_reference_retirement_target SET {field}=?1"),
            [&original],
        )
        .unwrap();
    }
    fixture.connection = reconnect(&fixture).unwrap();
}
