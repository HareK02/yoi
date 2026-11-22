//! Storage component tests: real retained migration functions and published SQL
//! on isolated SQLite databases; no authority, Runtime or domain recovery fakes.
//! The normal workspace-server lib test target runs this module. These fixtures
//! attest physical acceptance/data retention, not policy transfer or Job recovery.
use super::*;
use rusqlite::{backup::Backup, params};

#[test]
fn copied_reference_stages_match_independent_frozen_schema_construction() {
    let copied = build_references().unwrap();
    let stages = std::iter::once(Stage::Base)
        .chain(FAMILIES.into_iter().flat_map(|family| {
            (1..=family.names().len()).map(move |count| Stage::Published(family, count))
        }))
        .chain((119..=121).map(Stage::Converged));
    let base = snapshot(&reference_base().unwrap()).unwrap();
    let mut tables = BTreeSet::new();
    let mut new_columns: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let base_conn = reference_base().unwrap();
    let mut count = 0;
    for stage in stages {
        // Independent construction is the oracle: do not use the copy helper.
        let conn = reference_base().unwrap();
        install_reference(&conn, stage).unwrap();
        let shape = snapshot(&conn).unwrap();
        assert_eq!(copied.stages[&stage], shape, "stage {stage:?}");
        for (name, object) in &shape {
            if base.get(name) != Some(object) {
                tables.insert(object.1.clone());
            }
            if object.0 == "table" {
                let old: BTreeSet<_> = super::super::table_columns(&base_conn, name)
                    .unwrap()
                    .into_iter()
                    .collect();
                let fields: BTreeSet<_> = super::super::table_columns(&conn, name)
                    .unwrap()
                    .into_iter()
                    .collect();
                new_columns
                    .entry(name.clone())
                    .or_default()
                    .extend(fields.difference(&old).cloned());
            }
        }
        count += 1;
    }
    assert_eq!(copied.stages.len(), count);
    assert_eq!(copied.tables, tables);
    assert_eq!(copied.new_columns, new_columns);
}

#[test]
fn table_declaration_normalization_is_idempotent_after_extension() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE sample(z TEXT CHECK (z IN ('x','y')), a INTEGER PRIMARY KEY);")
        .unwrap();
    let mut shape = snapshot(&connection).unwrap();
    let tokens = &mut shape.get_mut("sample").unwrap().2;
    assert_eq!(declarations(tokens.clone()).unwrap(), *tokens);
    let start = tokens.iter().position(|token| token == "(").unwrap();
    tokens.splice(
        start + 1..start + 1,
        ["word:middle".into(), "word:text".into(), ",".into()],
    );
    let extended = declarations(tokens.clone()).unwrap();
    assert_eq!(declarations(extended.clone()).unwrap(), extended);
    connection
        .execute_batch("ALTER TABLE sample ADD COLUMN middle TEXT;")
        .unwrap();
    assert_eq!(snapshot(&connection).unwrap()["sample"].2, extended);
}

fn published_history_rows(
    stage: Stage,
    baseline: i64,
    predecessor: Option<(Family, usize)>,
) -> Vec<(i64, String)> {
    let mut rows = vec![(baseline, super::super::SCHEMA_BASELINE_NAME.to_owned())];
    let published = match stage {
        Stage::Published(family, count) => Some((family, count)),
        _ => predecessor,
    };
    if let Some((family, count)) = published {
        for (index, name) in family.names().iter().take(count).enumerate() {
            let version = 114 + index as i64;
            if version > baseline {
                rows.push((version, (*name).to_owned()));
            }
        }
    }
    if let Stage::Converged(current) = stage {
        for (version, name) in [
            (119, CONVERGED),
            (120, PRIOR_RELEASE),
            (121, CLEANUP_COVERAGE),
        ] {
            if version > baseline && version <= current {
                rows.push((version, (*name).to_owned()));
            }
        }
    }
    rows
}

fn save_history(
    conn: &Connection,
    stage: Stage,
    baseline: i64,
    predecessor: Option<(Family, usize)>,
) {
    conn.execute("DELETE FROM __yoi_schema_migrations", [])
        .unwrap();
    for (version, name) in published_history_rows(stage, baseline, predecessor) {
        conn.execute(
            "INSERT INTO __yoi_schema_migrations(version,name) VALUES(?1,?2)",
            params![version, name],
        )
        .unwrap();
    }
}

fn fixture(stage: Stage, baseline: i64, predecessor: Option<(Family, usize)>) -> Connection {
    let conn = reference_base().unwrap();
    install_reference(&conn, stage).unwrap();
    save_history(&conn, stage, baseline, predecessor);
    conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at)
        VALUES('owner','user','owner','Owner','1','1');
        INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at)
        VALUES('workspace-a','owner','Saved workspace','active','1','1');
        INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,created_at,updated_at,input_disposition)
        VALUES('workspace-a','saved-job','journal','digest','input','{}','fingerprint','pending',1,'1','1','erased');
        INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,input_digest,state,deadline_at,created_at,updated_at)
        VALUES('workspace-a','saved-job','saved-attempt',1,'digest','reserved','2','1','1');
        INSERT INTO worker_removal_operations(operation_id,plan_id,input_fingerprint,workspace_id,runtime_id,worker_id,worker_updated_at,policy_id,policy_digest,session_disposition,metadata_disposition,archive_retention_kind,diagnostics_disposition,blockers_json,state,reason,created_at,updated_at)
        VALUES('saved-removal','saved-plan','saved-fingerprint','workspace-a','runtime','worker','1','policy','digest','erase','erase','none','erase','[]','succeeded','saved','1','1');").unwrap();
    if super::super::table_exists(&conn, "permission_policy_receipts").unwrap() {
        conn.execute_batch("INSERT INTO permission_policy_receipts(workspace_id,operation_id,fingerprint,result_json)
            VALUES('workspace-a','saved-permission','saved-fingerprint','{\"saved\":true}');").unwrap();
    }
    if super::super::table_exists(&conn, "server_feature_root").unwrap() {
        conn.execute_batch("INSERT INTO server_feature_root(root_slot,reservation_id,request_fingerprint,state,accepted_receipt)
            VALUES(1,'saved-reservation','saved-fingerprint','accepted','saved-root-receipt');
            INSERT INTO content_history_targets(workspace,target,head,erased)
            VALUES('workspace-a','workspace-config',NULL,1);").unwrap();
    }
    conn
}

fn saved_rows(conn: &Connection) -> Vec<(String, Vec<String>)> {
    let names = conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap()
        .query_map([], |row| row.get::<_, String>(0)).unwrap()
        .collect::<rusqlite::Result<Vec<_>>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut statement = conn
                .prepare(&format!("SELECT * FROM \"{}\"", name.replace('"', "\"\"")))
                .unwrap();
            let count = statement.column_count();
            let mut rows = statement
                .query_map([], |row| {
                    (0..count)
                        .map(|index| row.get::<_, rusqlite::types::Value>(index))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .map(|row| format!("{:?}", row.unwrap()))
                .collect::<Vec<_>>();
            rows.sort();
            (name, rows)
        })
        .collect()
}

// Explicit canonical tail: schema 132 has no registered migration. Keep this
// expectation independent of published_plan and the production migration list.
fn expected_published_tail() -> [(i64, &'static str); 19] {
    [
        (122, crate::config_adoption::MIGRATION_NAME),
        (123, super::super::backend_job_upgrade::MIGRATION_NAME),
        (124, super::super::WORKER_CREATE_FAILURE_MIGRATION_NAME),
        (
            125,
            super::super::backend_job_recovery_store::CAUSE_MIGRATION_NAME,
        ),
        (126, super::super::SERVICE_OUTCOME_MIGRATION_NAME),
        (
            127,
            super::super::worker_input_handoff_store::MIGRATION_NAME,
        ),
        (128, super::super::TICKET_PLAN_RETIREMENT_MIGRATION_NAME),
        (
            129,
            super::super::DOMAIN_FAILURE_OBSERVATIONS_MIGRATION_NAME,
        ),
        (130, super::super::backend_job_current::MIGRATION_NAME),
        (131, crate::launch_policy::MIGRATION_NAME),
        (133, crate::subject_policy::MIGRATION_NAME),
        (134, super::super::backend_job_finalizing::MIGRATION_NAME),
        (
            135,
            super::super::subject_release_input_handoff_store::MIGRATION_NAME,
        ),
        (
            136,
            super::super::launch_policy_identity_migration::MIGRATION_NAME,
        ),
        (
            137,
            super::super::worker_lifecycle_notifications::MIGRATION_NAME,
        ),
        (
            138,
            super::super::notification_routing::merge_requests::MIGRATION_NAME,
        ),
        (
            139,
            super::super::backend_job_result_application::MIGRATION_NAME,
        ),
        (
            140,
            super::super::backend_job_recording_input::MIGRATION_NAME,
        ),
        (141, super::super::EXTERNAL_WORKDIR_SESSION_MIGRATION_NAME),
    ]
}

fn assert_accepted_without_domain_changes(conn: &Connection, current: i64) {
    let rows = saved_rows(conn);
    let shape = snapshot(conn).unwrap();
    verify_history(conn, current).unwrap();
    if current >= 114 {
        let plan = published_plan(conn, current).unwrap().unwrap();
        assert_eq!(plan.current_schema_version, current);
        assert_eq!(
            plan.target_schema_version,
            super::super::LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            plan.migrations
                .iter()
                .map(|step| (step.version, step.name.as_str()))
                .collect::<Vec<_>>(),
            expected_published_tail()
        );
    }
    assert_eq!(rows, saved_rows(conn));
    assert_eq!(shape, snapshot(conn).unwrap());
}

#[test]
fn fresh_published_baselines_require_their_real_shape_and_named_receipts_cannot_substitute_it() {
    for family in FAMILIES {
        for count in 1..=family.names().len() {
            let stage = Stage::Published(family, count);
            let current = 113 + count as i64;
            let conn = fixture(stage, current, None);
            assert_accepted_without_domain_changes(&conn, current);
            // A baseline alone is ambiguous; a named receipt from another family
            // must not make this physical database acceptable to the planner.
            let shape = snapshot(&conn).unwrap();
            for other in FAMILIES
                .into_iter()
                .filter(|other| *other != family && count <= other.names().len())
            {
                save_history(&conn, Stage::Published(other, count), 113, None);
                let rows = saved_rows(&conn);
                assert!(
                    published_plan(&conn, current).is_err(),
                    "{stage:?}, {other:?}"
                );
                assert_eq!(saved_rows(&conn), rows);
                assert_eq!(snapshot(&conn).unwrap(), shape);
            }
        }
    }
}

#[test]
fn fresh_converged_shapes_keep_erasure_fences_and_job_state_without_writes() {
    for current in 119..=121 {
        let conn = fixture(Stage::Converged(current), current, None);
        assert_accepted_without_domain_changes(&conn, current);
        let erased: i64 = conn
            .query_row("SELECT erased FROM content_history_targets", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(erased, 1);
        // A physical check must not act as the Job owner's new Failed migration.
        let state: String = conn
            .query_row("SELECT state FROM backend_jobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(state, "pending");
    }
}

// Snapshot retained values, excluding the execution conversion fields and new nullable columns.
// Obsolete Job execution history is discarded; unrelated published values remain exact.
fn immutable_rows(conn: &Connection) -> Vec<(String, Vec<String>)> {
    let mut result = Vec::new();
    for (name, _) in saved_rows(conn) {
        if name == "__yoi_schema_migrations"
            || super::super::backend_job_current::OBSOLETE_TABLES.contains(&name.as_str())
            || ["backend_jobs", "backend_job_attempts"].contains(&name.as_str())
        {
            continue;
        }
        if name == "typed_ticket_orchestration_plans" {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM typed_ticket_orchestration_plans",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0
            );
            continue; // Ticket17 retires this empty fixture table.
        }
        let columns = super::super::table_columns(conn, &name)
            .unwrap()
            .into_iter()
            .filter(|column| {
                !["backend_jobs", "backend_job_attempts"].contains(&name.as_str())
                    || ![
                        "state",
                        "failure_category",
                        "failure_detail",
                        "allocation_precluded_at",
                        "failure_cause",
                        "worker_cleanup_failure_cause",
                    ]
                    .contains(&column.as_str())
            })
            .filter(|column| name != "plugin_service_consumers" || column != "last_outcome_json")
            .filter(|column| {
                name != "worker_removal_operations" || column != "journal_removal_json"
            })
            .filter(|column| {
                name != "worker_create_reservations" || column != "startup_failure_json"
            })
            .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
            .collect::<Vec<_>>();
        let columns = columns.join(",");
        let predicate = if name == "ticket_schema_migrations" {
            " WHERE version<=16"
        } else {
            ""
        };
        let mut statement = conn
            .prepare(&format!("SELECT {columns} FROM \"{name}\"{predicate}"))
            .unwrap();
        let count = statement.column_count();
        let mut rows = statement
            .query_map([], |row| {
                (0..count)
                    .map(|index| row.get::<_, rusqlite::types::Value>(index))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .map(|row| format!("{:?}", row.unwrap()))
            .collect::<Vec<_>>();
        rows.sort();
        result.push((name, rows));
    }
    result
}

fn receipt_values_through(conn: &Connection, version: i64) -> Vec<(i64, String, String)> {
    conn.prepare("SELECT version,name,applied_at FROM __yoi_schema_migrations WHERE version<=?1 ORDER BY version")
        .unwrap()
        .query_map([version], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn exercise_published_runner(stage: Stage, baseline: i64, predecessor: Option<(Family, usize)>) {
    let source = fixture(stage, baseline, predecessor);
    let expected = immutable_rows(&source);
    let original_receipts = super::super::workspace_schema_migration_history(&source).unwrap();
    let original_version = original_receipts.last().unwrap().0;
    let original_receipt_values = receipt_values_through(&source, original_version);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("catalog.db");
    let mut destination = Connection::open(&path).unwrap();
    Backup::new(&source, &mut destination)
        .unwrap()
        .run_to_completion(128, std::time::Duration::ZERO, None)
        .unwrap();
    drop(destination);
    let mut committed_history = None;
    for _ in 0..2 {
        let conn = Connection::open(&path).unwrap();
        // Actual production open/ordered runner, including the new migration.
        super::super::prepare_connection(&conn).unwrap();
        assert_eq!(
            super::super::current_schema_version(&conn).unwrap(),
            super::super::LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM backend_job_attempts WHERE attempt_id='saved-attempt'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        super::super::backend_job_current::verify(&conn).unwrap();
        let actual = immutable_rows(&conn);
        for (table, rows) in &expected {
            assert_eq!(
                &actual.iter().find(|(name, _)| name == table).unwrap().1,
                rows,
                "{table}, from {stage:?}, baseline {baseline}"
            );
        }
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM backend_jobs WHERE job_id='saved-job'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        let mut history = super::super::workspace_schema_migration_history(&conn).unwrap();
        for (version, name) in expected_published_tail().into_iter().rev() {
            assert_eq!(history.pop(), Some((version, name.into())));
        }
        assert_eq!(history, original_receipts);
        assert_eq!(
            receipt_values_through(&conn, original_version),
            original_receipt_values,
            "receipt prefix, including applied_at, must remain unchanged"
        );
        verify_history(&conn, super::super::LATEST_SCHEMA_VERSION).unwrap();
        let saved = saved_rows(&conn);
        if let Some(previous) = &committed_history {
            assert_eq!(previous, &saved);
        } else {
            committed_history = Some(saved);
        }
    }
}

#[test]
fn published_database_runs_normal_store_preparation_and_reopens_without_downgrade() {
    // Every physical stage still runs the real migrations and disk reopen. Only
    // receipt/baseline permutations move to the pure matrix below: they select
    // the same canonical tail and do not alter the fixture's schema or data.
    for family in FAMILIES {
        for count in 1..=family.names().len() {
            // Baseline 114 leaves all three families possible. Exercise both
            // non-first candidates through migration and reopen, where reference
            // shapes also include later extensions, not just initial acceptance.
            // Later stages retain named prefixes for these families.
            let baseline = match (family, count) {
                (Family::Removal | Family::Jobs, 1) => 114,
                _ => 113,
            };
            exercise_published_runner(Stage::Published(family, count), baseline, None);
        }
    }
    // Retain each family's named predecessor receipts through convergence, too.
    for (current, predecessor) in [
        (119, (Family::Permissions, 2)),
        (120, (Family::Removal, 4)),
        (121, (Family::Jobs, 5)),
    ] {
        exercise_published_runner(Stage::Converged(current), 113, Some(predecessor));
    }
}

fn published_histories() -> Vec<(Stage, i64, Option<(Family, usize)>)> {
    let mut histories = Vec::new();
    for family in FAMILIES {
        for count in 1..=family.names().len() {
            let stage = Stage::Published(family, count);
            for baseline in 113..=113 + count as i64 {
                histories.push((stage, baseline, None));
            }
        }
    }
    for current in 119..=121 {
        for baseline in 119..=current {
            histories.push((Stage::Converged(current), baseline, None));
        }
        for origin in
            std::iter::once(None).chain(FAMILIES.into_iter().flat_map(|family| {
                (1..=family.names().len()).map(move |count| Some((family, count)))
            }))
        {
            histories.push((Stage::Converged(current), 113, origin));
        }
    }
    histories
}

// Exercise the production receipt recognizers without constructing SQLite.
// This composes receipt rules only; the runner/shape tests own their DB wiring.
fn published_origin_stages(
    mut current: i64,
    mut rows: &[(i64, String)],
) -> Result<BTreeSet<Stage>> {
    if current >= 124 {
        verify_failure_receipts(current, rows, FailureTail::Canonical)?;
        let end = rows
            .iter()
            .position(|row| row.0 >= 124)
            .unwrap_or(rows.len());
        rows = &rows[..end];
        current = 123;
    }
    while current >= 122 {
        rows = pre_failure_receipt_prefix(current, rows)?
            .ok_or_else(|| invalid("matrix requires a retained published origin"))?;
        current = rows.last().unwrap().0;
    }
    let stages: BTreeSet<_> = retained_receipt_stages(current, rows)?
        .into_iter()
        .collect();
    if stages.is_empty() {
        return Err(invalid("no matching published receipts"));
    }
    Ok(stages)
}

fn expected_origin_stages(stage: Stage, baseline: i64, current: i64) -> BTreeSet<Stage> {
    if let Stage::Published(_, count) = stage {
        if baseline == current {
            // Independent expectation for ambiguous branch-local baseline names.
            let families: &[Family] = match current {
                114 | 115 => &[Family::Permissions, Family::Removal, Family::Jobs],
                116 | 117 => &[Family::Removal, Family::Jobs],
                118 => &[Family::Jobs],
                _ => panic!("unexpected published baseline {current}"),
            };
            return families
                .iter()
                .map(|family| Stage::Published(*family, count))
                .collect();
        }
    }
    BTreeSet::from([stage])
}

#[test]
fn published_receipt_matrix_preserves_prefixes_and_plans_only_the_missing_tail() {
    let histories = published_histories();
    assert_eq!(histories.len(), 81, "retained receipt input matrix");
    for (stage, baseline, predecessor) in histories {
        let original = published_history_rows(stage, baseline, predecessor);
        let current = original.last().unwrap().0;
        let expected_stages = expected_origin_stages(stage, baseline, current);
        assert_eq!(
            published_origin_stages(current, &original).unwrap(),
            expected_stages
        );
        let plan = FailureTail::Canonical.plan(current);
        assert_eq!(plan.current_schema_version, current);
        assert_eq!(
            plan.target_schema_version,
            super::super::LATEST_SCHEMA_VERSION
        );
        assert_eq!(
            plan.migrations
                .iter()
                .map(|step| (step.version, step.name.as_str()))
                .collect::<Vec<_>>(),
            expected_published_tail()
        );
        // This is a receipt fixture, not a fake migration. Every intermediate
        // appended prefix must remain recognizable, with no invented 114..121
        // rows and with the exact independent canonical suffix. Real insertion
        // and saved prefix preservation are checked by the 14-stage runner.
        let mut appended = original.clone();
        for (index, step) in plan.migrations.iter().enumerate() {
            appended.push((step.version, step.name.clone()));
            if step.version == 122 || step.version == 123 {
                let prefix = pre_failure_receipt_prefix(step.version, &appended).unwrap();
                assert_eq!(prefix, Some(&appended[..original.len() + index]));
                if step.version == 122 {
                    assert_eq!(prefix, Some(original.as_slice()));
                }
            }
            assert_eq!(
                appended[original.len()..]
                    .iter()
                    .map(|(version, name)| (*version, name.as_str()))
                    .collect::<Vec<_>>(),
                expected_published_tail()[..=index]
            );
            assert_eq!(
                published_origin_stages(step.version, &appended).unwrap(),
                expected_stages,
                "{stage:?}, baseline {baseline}, predecessor {predecessor:?}, at {}",
                step.version
            );
            let remaining = FailureTail::Canonical.plan(step.version);
            assert_eq!(
                remaining
                    .migrations
                    .iter()
                    .map(|step| (step.version, step.name.as_str()))
                    .collect::<Vec<_>>(),
                expected_published_tail()[index + 1..]
            );
        }
    }
}

#[test]
fn published_receipt_matrix_rejects_damaged_appended_tail() {
    for (stage, baseline, predecessor) in published_histories() {
        let mut rows = published_history_rows(stage, baseline, predecessor);
        for (current, name) in expected_published_tail() {
            rows.push((current, name.to_owned()));
            let mut damaged = rows.clone();
            damaged.last_mut().unwrap().1 = "unknown receipt".into();
            assert!(published_origin_stages(current, &damaged).is_err());
            damaged = rows.clone();
            damaged.pop();
            assert!(published_origin_stages(current, &damaged).is_err());
            damaged = rows.clone();
            damaged.push(rows.last().unwrap().clone());
            assert!(published_origin_stages(current, &damaged).is_err());
            // A later correct receipt must not hide damage to an earlier bridge
            // or tail receipt. Keep original histories valid: some shortened
            // published prefixes are themselves legitimate converged origins.
            for index in rows.iter().position(|row| row.0 == 122).unwrap()..rows.len() {
                damaged = rows.clone();
                damaged[index].1 = "counterfeit predecessor".into();
                assert!(published_origin_stages(current, &damaged).is_err());
            }
        }
    }
}

#[test]
fn retained_receipt_recognition_rejects_unknown_mixed_missing_or_reordered_rows() {
    let rows = published_history_rows(Stage::Published(Family::Jobs, 5), 113, None);
    assert_eq!(
        published_origin_stages(118, &rows).unwrap(),
        BTreeSet::from([Stage::Published(Family::Jobs, 5)])
    );
    let mut invalid_histories = vec![Vec::new()];
    for baseline in [49, 119, 122] {
        let mut changed = rows.clone();
        changed[0].0 = baseline;
        invalid_histories.push(changed);
    }
    for index in 0..rows.len() {
        let mut changed = rows.clone();
        changed[index].1 = "unknown receipt".into();
        invalid_histories.push(changed);
        let mut changed = rows.clone();
        changed.remove(index);
        invalid_histories.push(changed);
        let mut changed = rows.clone();
        changed.insert(index, rows[index].clone());
        invalid_histories.push(changed);
    }
    let mut mixed = rows.clone();
    mixed[2].1 = Family::Removal.names()[1].to_owned();
    invalid_histories.push(mixed);
    let mut reordered = rows.clone();
    reordered.swap(1, 2);
    invalid_histories.push(reordered);
    let mut suffix = rows.clone();
    suffix.push((119, "unknown suffix".into()));
    invalid_histories.push(suffix);
    for changed in invalid_histories {
        assert!(
            published_origin_stages(118, &changed).is_err(),
            "{changed:?}"
        );
    }
    assert!(published_origin_stages(117, &rows).is_err());
    assert!(published_origin_stages(119, &rows).is_err());
}

#[test]
fn retained_receipts_keep_both_schema_103_identities_and_baseline_bounds() {
    for name in [
        "Passkey metadata and atomic addition results",
        "Job input retirement independent of immutable receipts",
    ] {
        let rows = vec![
            (102, super::super::SCHEMA_BASELINE_NAME.to_owned()),
            (103, name.to_owned()),
        ];
        assert!(retained_receipt_stages(103, &rows).unwrap().is_empty());
        let mut counterfeit = rows;
        counterfeit[1].1 = "counterfeit schema-103 identity".into();
        assert!(retained_receipt_stages(103, &counterfeit).is_err());
    }
    for current in [50, 103, 113] {
        let rows = [(current, super::super::SCHEMA_BASELINE_NAME.to_owned())];
        let expected = if current == 113 {
            vec![Stage::Base]
        } else {
            Vec::new()
        };
        assert_eq!(retained_receipt_stages(current, &rows).unwrap(), expected);
    }
    for current in [49, 122] {
        let rows = [(current, super::super::SCHEMA_BASELINE_NAME.to_owned())];
        assert!(retained_receipt_stages(current, &rows).is_err());
    }
}

#[test]
fn bridge_receipts_distinguish_fresh_baselines_and_require_exact_predecessors() {
    for (current, name, wrong_prior) in [
        (122, crate::config_adoption::MIGRATION_NAME, 112),
        (123, super::super::backend_job_upgrade::MIGRATION_NAME, 121),
    ] {
        let fresh = [(current, super::super::SCHEMA_BASELINE_NAME.to_owned())];
        assert!(
            pre_failure_receipt_prefix(current, &fresh)
                .unwrap()
                .is_none()
        );
        let missing = [(current, name.to_owned())];
        assert!(pre_failure_receipt_prefix(current, &missing).is_err());
        let wrong = [
            (wrong_prior, super::super::SCHEMA_BASELINE_NAME.to_owned()),
            (current, name.to_owned()),
        ];
        assert!(pre_failure_receipt_prefix(current, &wrong).is_err());
    }
}

#[test]
fn unknown_mixed_missing_receipts_and_shapes_are_diagnosed_without_writes() {
    for (sql, current) in [
        (
            "UPDATE __yoi_schema_migrations SET name='unknown receipt' WHERE version=115",
            118,
        ),
        ("DELETE FROM __yoi_schema_migrations WHERE version=116", 118),
        (
            "UPDATE __yoi_schema_migrations SET name='Worker removal expected Feature database identity' WHERE version=115",
            118,
        ),
        ("DROP TRIGGER backend_job_cleanup_owner_immutable", 118),
        (
            "CREATE INDEX unreceipted_job_index ON backend_jobs(job_id)",
            118,
        ),
        (
            "INSERT INTO __yoi_schema_migrations(version,name) VALUES(122,'unknown future')",
            122,
        ),
    ] {
        let conn = fixture(Stage::Published(Family::Jobs, 5), 113, None);
        conn.execute_batch(sql).unwrap();
        let rows = saved_rows(&conn);
        let shape = snapshot(&conn).unwrap();
        assert!(verify_history(&conn, current).is_err(), "{sql}");
        assert_eq!(saved_rows(&conn), rows);
        assert_eq!(snapshot(&conn).unwrap(), shape);
    }
    let conn = fixture(Stage::Published(Family::Permissions, 2), 113, None);
    conn.execute_batch(COLLECTION_SQL).unwrap();
    assert!(
        verify_history(&conn, 115)
            .unwrap_err()
            .to_string()
            .contains("mismatch")
    );
    let conn = fixture(Stage::Converged(121), 113, None);
    conn.execute_batch("DROP TRIGGER backend_job_cleanup_origin_immutable")
        .unwrap();
    assert!(verify_history(&conn, 121).is_err());
}

#[test]
fn missing_or_weakened_constraints_and_literal_changes_are_not_shape_equivalent() {
    let conn = fixture(Stage::Published(Family::Permissions, 1), 113, None);
    conn.execute_batch("DROP TABLE permission_policy_receipts;
        CREATE TABLE permission_policy_receipts(workspace_id TEXT NOT NULL,operation_id TEXT NOT NULL,fingerprint TEXT NOT NULL,result_json TEXT NOT NULL,PRIMARY KEY(workspace_id,operation_id));").unwrap();
    assert!(verify_history(&conn, 114).is_err());
    let conn = fixture(Stage::Published(Family::Jobs, 5), 113, None);
    conn.execute_batch("DROP TRIGGER backend_job_cleanup_owner_immutable;
        CREATE TRIGGER backend_job_cleanup_owner_immutable BEFORE UPDATE ON backend_job_cleanup_invocation_owners
        BEGIN SELECT RAISE(ABORT,'Cleanup invocation ownership is immutable'); END;").unwrap();
    assert!(verify_history(&conn, 118).is_err());
}

fn retained_to(target: i64, alternate_103: bool) -> (tempfile::TempDir, Connection) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("retained.db");
    super::super::tests::prepare_schema_v50(&path, Some("workspace-a"));
    let conn = Connection::open(&path).unwrap();
    super::super::configure_sqlite(&conn).unwrap();
    for migration in super::super::MIGRATIONS
        .iter()
        .filter(|migration| migration.version <= target)
    {
        let current = super::super::current_schema_version(&conn).unwrap();
        verify_history(&conn, current).unwrap();
        let migration = if migration.version == 103 && alternate_103 {
            &super::super::JOB_INPUT_RETENTION_SCHEMA_103
        } else {
            migration
        };
        (migration.apply)(&conn).unwrap();
    }
    verify_history(&conn, target).unwrap();
    (temp, conn)
}

#[test]
fn oldest_data_bearing_schema_and_both_real_103_histories_migrate_without_relabeling() {
    for alternate in [false, true] {
        let (_temp, conn) = retained_to(103, alternate);
        let original = super::super::workspace_schema_migration_history(&conn).unwrap();
        super::super::prepare_connection(&conn).unwrap();
        verify_history(&conn, super::super::LATEST_SCHEMA_VERSION).unwrap();
        let updated = super::super::workspace_schema_migration_history(&conn).unwrap();
        assert_eq!(&updated[..original.len()], &original);
        let name: String = conn
            .query_row(
                "SELECT display_name FROM workspaces WHERE workspace_id='workspace-a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Workspace A");
    }
}

#[test]
fn fresh_103_baselines_use_shape_and_never_repair_missing_receipts_or_guards() {
    for alternate in [false, true] {
        let (_temp, conn) = retained_to(103, alternate);
        conn.execute_batch("DELETE FROM __yoi_schema_migrations;
            INSERT INTO __yoi_schema_migrations(version,name) VALUES(103,'workspace schema baseline');").unwrap();
        assert_accepted_without_domain_changes(&conn, 103);
        if alternate {
            conn.execute_batch("DROP TRIGGER backend_job_input_retirement")
                .unwrap();
        } else {
            conn.execute_batch("DROP TABLE passkey_addition_receipts")
                .unwrap();
        }
        let rows = saved_rows(&conn);
        assert!(verify_history(&conn, 103).is_err());
        assert_eq!(saved_rows(&conn), rows);
    }
    let (_temp, conn) = retained_to(102, false);
    conn.execute_batch(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(103,'workspace schema baseline')",
    )
    .unwrap();
    assert!(verify_history(&conn, 103).is_err());
}

#[test]
fn later_family_objects_without_receipts_and_half_converged_103_are_rejected() {
    let conn = fixture(Stage::Base, 113, None);
    conn.execute_batch(CLAIMS_SQL).unwrap();
    assert!(verify_history(&conn, 113).is_err());
    let (_temp, conn) = retained_to(103, false);
    conn.execute_batch("ALTER TABLE backend_jobs ADD COLUMN input_disposition TEXT NOT NULL DEFAULT 'available' CHECK(input_disposition IN ('available','reclaimed','erased','invalidated'))").unwrap();
    assert!(verify_history(&conn, 103).is_err());
}

#[test]
fn schema_comparison_preserves_literals_programs_and_composite_key_order() {
    let left = declarations(
        tokens("CREATE TABLE item(a TEXT, b TEXT, PRIMARY KEY(a,b), CHECK(a='Sensitive'))")
            .unwrap(),
    )
    .unwrap();
    let reordered = declarations(
        tokens("CREATE TABLE \"item\"(b TEXT, CHECK(a='Sensitive'), a TEXT, PRIMARY KEY(a,b))")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(left, reordered);
    for sql in [
        "CREATE TABLE item(a TEXT, b TEXT, PRIMARY KEY(b,a), CHECK(a='Sensitive'))",
        "CREATE TABLE item(a TEXT, b TEXT, PRIMARY KEY(a,b), CHECK(a='sensitive'))",
    ] {
        assert_ne!(left, declarations(tokens(sql).unwrap()).unwrap());
    }
}
