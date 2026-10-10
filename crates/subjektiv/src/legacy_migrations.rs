//! Frozen migrations 1–6 and the one-time conversion of their data.
//! Do not expose these old fields through current domain operations.
use super::*;

pub(super) fn create_schema(transaction: &Transaction<'_>) -> feature_storage::Result<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE store_scope (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    workspace_id TEXT NOT NULL UNIQUE
);

CREATE TABLE subjects (
    subject_id TEXT PRIMARY KEY,
    role TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('active', 'retired')),
    store_revision INTEGER NOT NULL CHECK (store_revision >= 0),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE staging_records (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'preference', 'working_assumption', 'constraint',
            'decision', 'open_question', 'lesson'
        )
    ),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, candidate_id),
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT
);

CREATE TABLE memory_records (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    current_revision INTEGER NOT NULL CHECK (current_revision > 0),
    kind TEXT NOT NULL CHECK (
        kind IN (
            'preference', 'working_assumption', 'constraint',
            'decision', 'open_question', 'lesson'
        )
    ),
    state TEXT NOT NULL CHECK (state IN ('active', 'resolved', 'retracted')),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, memory_id),
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, memory_id, current_revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE memory_revisions (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    kind TEXT NOT NULL CHECK (
        kind IN (
            'preference', 'working_assumption', 'constraint',
            'decision', 'open_question', 'lesson'
        )
    ),
    state TEXT NOT NULL CHECK (state IN ('active', 'resolved', 'retracted')),
    record_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, memory_id, revision),
    FOREIGN KEY (subject_id, memory_id)
        REFERENCES memory_records(subject_id, memory_id)
        ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE memory_revision_candidates (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    candidate_id TEXT NOT NULL,
    PRIMARY KEY (subject_id, memory_id, revision, candidate_id),
    FOREIGN KEY (subject_id, memory_id, revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_records(subject_id, candidate_id) ON DELETE RESTRICT
);

CREATE TABLE memory_revision_derivations (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    source_memory_id TEXT NOT NULL,
    source_revision INTEGER NOT NULL,
    PRIMARY KEY (
        subject_id, memory_id, revision, source_memory_id, source_revision
    ),
    FOREIGN KEY (subject_id, memory_id, revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, source_memory_id, source_revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT
);

CREATE TABLE staging_resolutions (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    resolution_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK (
        action IN ('applied', 'discarded', 'invalid', 'duplicate', 'already_covered')
    ),
    reason TEXT NOT NULL,
    affected_refs_json TEXT NOT NULL,
    staging_raw_json TEXT NOT NULL,
    resolution_json TEXT NOT NULL,
    resolved_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, candidate_id),
    UNIQUE (resolution_id),
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_records(subject_id, candidate_id) ON DELETE RESTRICT
);

CREATE TABLE staging_resolution_targets (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (subject_id, candidate_id, memory_id, revision),
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_resolutions(subject_id, candidate_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, memory_id, revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT
);

CREATE TABLE surface_snapshots (
    subject_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    built_from_store_revision INTEGER NOT NULL CHECK (built_from_store_revision >= 0),
    snapshot_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, snapshot_id),
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT
);

CREATE TABLE surface_snapshot_refs (
    subject_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (subject_id, snapshot_id, memory_id),
    FOREIGN KEY (subject_id, snapshot_id)
        REFERENCES surface_snapshots(subject_id, snapshot_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, memory_id, revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT
);

CREATE TABLE memory_revision_seals (
    subject_id TEXT NOT NULL,
    memory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (subject_id, memory_id, revision),
    FOREIGN KEY (subject_id, memory_id, revision)
        REFERENCES memory_revisions(subject_id, memory_id, revision) ON DELETE RESTRICT
);

CREATE TABLE staging_resolution_seals (
    subject_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    PRIMARY KEY (subject_id, candidate_id),
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_resolutions(subject_id, candidate_id) ON DELETE RESTRICT
);

CREATE TABLE surface_snapshot_seals (
    subject_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    PRIMARY KEY (subject_id, snapshot_id),
    FOREIGN KEY (subject_id, snapshot_id)
        REFERENCES surface_snapshots(subject_id, snapshot_id) ON DELETE RESTRICT
);

CREATE TRIGGER memory_revision_candidates_no_late_insert
BEFORE INSERT ON memory_revision_candidates
WHEN EXISTS (
    SELECT 1 FROM memory_revision_seals
    WHERE subject_id = NEW.subject_id
      AND memory_id = NEW.memory_id
      AND revision = NEW.revision
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv memory history evidence is sealed');
END;
CREATE TRIGGER memory_revision_derivations_no_late_insert
BEFORE INSERT ON memory_revision_derivations
WHEN EXISTS (
    SELECT 1 FROM memory_revision_seals
    WHERE subject_id = NEW.subject_id
      AND memory_id = NEW.memory_id
      AND revision = NEW.revision
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv derivations are sealed');
END;
CREATE TRIGGER staging_resolution_targets_no_late_insert
BEFORE INSERT ON staging_resolution_targets
WHEN EXISTS (
    SELECT 1 FROM staging_resolution_seals
    WHERE subject_id = NEW.subject_id
      AND candidate_id = NEW.candidate_id
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution targets are sealed');
END;
CREATE TRIGGER surface_snapshot_refs_no_late_insert
BEFORE INSERT ON surface_snapshot_refs
WHEN EXISTS (
    SELECT 1 FROM surface_snapshot_seals
    WHERE subject_id = NEW.subject_id
      AND snapshot_id = NEW.snapshot_id
) BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface references are sealed');
END;

CREATE TRIGGER memory_revision_seals_no_update
BEFORE UPDATE ON memory_revision_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv memory history seals are immutable');
END;
CREATE TRIGGER memory_revision_seals_no_delete
BEFORE DELETE ON memory_revision_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv memory history seals are retained');
END;
CREATE TRIGGER staging_resolution_seals_no_update
BEFORE UPDATE ON staging_resolution_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution seals are immutable');
END;
CREATE TRIGGER staging_resolution_seals_no_delete
BEFORE DELETE ON staging_resolution_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution seals are retained');
END;
CREATE TRIGGER surface_snapshot_seals_no_update
BEFORE UPDATE ON surface_snapshot_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv snapshot seals are immutable');
END;
CREATE TRIGGER surface_snapshot_seals_no_delete
BEFORE DELETE ON surface_snapshot_seals BEGIN
    SELECT RAISE(ABORT, 'subjektiv snapshot seals are retained');
END;

CREATE TRIGGER memory_revisions_no_update
BEFORE UPDATE ON memory_revisions BEGIN
    SELECT RAISE(ABORT, 'subjektiv memory history entries are immutable');
END;
CREATE TRIGGER memory_revisions_no_delete
BEFORE DELETE ON memory_revisions BEGIN
    SELECT RAISE(ABORT, 'subjektiv memory history entries are retained');
END;
CREATE TRIGGER memory_revision_candidates_no_update
BEFORE UPDATE ON memory_revision_candidates BEGIN
    SELECT RAISE(ABORT, 'subjektiv memory history evidence is immutable');
END;
CREATE TRIGGER memory_revision_candidates_no_delete
BEFORE DELETE ON memory_revision_candidates BEGIN
    SELECT RAISE(ABORT, 'subjektiv memory history evidence is retained');
END;
CREATE TRIGGER memory_revision_derivations_no_update
BEFORE UPDATE ON memory_revision_derivations BEGIN
    SELECT RAISE(ABORT, 'subjektiv derivations are immutable');
END;
CREATE TRIGGER memory_revision_derivations_no_delete
BEFORE DELETE ON memory_revision_derivations BEGIN
    SELECT RAISE(ABORT, 'subjektiv derivations are retained');
END;
CREATE TRIGGER staging_records_no_update
BEFORE UPDATE ON staging_records BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging records are immutable');
END;
CREATE TRIGGER staging_records_no_delete
BEFORE DELETE ON staging_records BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging records are retained');
END;
CREATE TRIGGER staging_resolutions_no_update
BEFORE UPDATE ON staging_resolutions BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging resolutions are immutable');
END;
CREATE TRIGGER staging_resolutions_no_delete
BEFORE DELETE ON staging_resolutions BEGIN
    SELECT RAISE(ABORT, 'subjektiv staging resolutions are retained');
END;
CREATE TRIGGER staging_resolution_targets_no_update
BEFORE UPDATE ON staging_resolution_targets BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution targets are immutable');
END;
CREATE TRIGGER staging_resolution_targets_no_delete
BEFORE DELETE ON staging_resolution_targets BEGIN
    SELECT RAISE(ABORT, 'subjektiv resolution targets are retained');
END;
CREATE TRIGGER surface_snapshots_no_update
BEFORE UPDATE ON surface_snapshots BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface snapshots are immutable');
END;
CREATE TRIGGER surface_snapshots_no_delete
BEFORE DELETE ON surface_snapshots BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface snapshots are retained');
END;
CREATE TRIGGER surface_snapshot_refs_no_update
BEFORE UPDATE ON surface_snapshot_refs BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface references are immutable');
END;
CREATE TRIGGER surface_snapshot_refs_no_delete
BEFORE DELETE ON surface_snapshot_refs BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface references are retained');
END;
"#,
    )?;
    Ok(())
}

pub(super) fn add_subject_session_attribution(
    transaction: &Transaction<'_>,
) -> feature_storage::Result<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE subject_session_attributions (
    session_id TEXT PRIMARY KEY,
    subject_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    attributed_at TEXT NOT NULL,
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT
);

CREATE INDEX subject_session_attributions_by_subject
ON subject_session_attributions(subject_id, attributed_at, session_id);

CREATE TRIGGER subject_session_attributions_no_update
BEFORE UPDATE ON subject_session_attributions BEGIN
    SELECT RAISE(ABORT, 'subjektiv session attribution is immutable');
END;
CREATE TRIGGER subject_session_attributions_no_delete
BEFORE DELETE ON subject_session_attributions BEGIN
    SELECT RAISE(ABORT, 'subjektiv session attribution is retained');
END;
"#,
    )?;
    Ok(())
}

pub(super) fn add_candidate_decision_receipts(
    transaction: &Transaction<'_>,
) -> feature_storage::Result<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE candidate_decision_receipts (
    subject_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    candidate_id TEXT NOT NULL,
    request_json TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, request_id),
    UNIQUE (subject_id, candidate_id),
    FOREIGN KEY (subject_id, candidate_id)
        REFERENCES staging_resolutions(subject_id, candidate_id) ON DELETE RESTRICT
);

CREATE TRIGGER candidate_decision_receipts_no_update
BEFORE UPDATE ON candidate_decision_receipts BEGIN
    SELECT RAISE(ABORT, 'subjektiv candidate decision receipts are immutable');
END;
CREATE TRIGGER candidate_decision_receipts_no_delete
BEFORE DELETE ON candidate_decision_receipts BEGIN
    SELECT RAISE(ABORT, 'subjektiv candidate decision receipts are retained');
END;
"#,
    )?;
    Ok(())
}

pub(super) fn add_surface_generation_state(
    transaction: &Transaction<'_>,
) -> feature_storage::Result<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE surface_generation_state (
    subject_id TEXT PRIMARY KEY,
    store_revision INTEGER NOT NULL CHECK (store_revision >= 0),
    status TEXT NOT NULL CHECK (status IN ('dirty', 'failed', 'ready')),
    snapshot_id TEXT,
    reason_code TEXT,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT,
    FOREIGN KEY (subject_id, snapshot_id)
        REFERENCES surface_snapshots(subject_id, snapshot_id) ON DELETE RESTRICT
);

INSERT INTO surface_generation_state (
    subject_id, store_revision, status, snapshot_id, reason_code, updated_at
)
SELECT subjects.subject_id,
       subjects.store_revision,
       'dirty',
       NULL,
       'legacy_snapshot_requires_regeneration',
       subjects.updated_at
FROM subjects
WHERE EXISTS (
    SELECT 1 FROM surface_snapshots any_snapshot
    WHERE any_snapshot.subject_id = subjects.subject_id
);

CREATE TABLE surface_generation_runs (
    subject_id TEXT NOT NULL,
    generation_id TEXT NOT NULL,
    store_revision INTEGER NOT NULL CHECK (store_revision >= 0),
    active_memory_count INTEGER NOT NULL CHECK (active_memory_count >= 0),
    materials_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (subject_id, generation_id),
    FOREIGN KEY (subject_id) REFERENCES subjects(subject_id) ON DELETE RESTRICT
);

CREATE INDEX surface_generation_runs_by_revision
ON surface_generation_runs(subject_id, store_revision, created_at, generation_id);

CREATE TRIGGER surface_generation_runs_no_update
BEFORE UPDATE ON surface_generation_runs BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface generation inputs are immutable');
END;
CREATE TRIGGER surface_generation_runs_no_delete
BEFORE DELETE ON surface_generation_runs BEGIN
    SELECT RAISE(ABORT, 'subjektiv surface generation inputs are retained');
END;
"#,
    )?;
    Ok(())
}

pub(super) fn add_subject_behavior(transaction: &Transaction<'_>) -> feature_storage::Result<()> {
    transaction.execute_batch(
        r#"
ALTER TABLE subjects ADD COLUMN behavior_md TEXT NOT NULL DEFAULT '';
ALTER TABLE subjects ADD COLUMN behavior_revision INTEGER NOT NULL DEFAULT 0
    CHECK (behavior_revision >= 0);
"#,
    )?;
    Ok(())
}

pub(super) fn add_surface_job_provenance(
    transaction: &Transaction<'_>,
) -> feature_storage::Result<()> {
    transaction.execute_batch("ALTER TABLE surface_generation_runs ADD COLUMN job_id TEXT; ALTER TABLE surface_generation_runs ADD COLUMN attempt_id TEXT;")?;
    Ok(())
}

/// Rebuilds the domain schema atomically, giving every retained historical
/// Memory change a fresh opaque identity and an explicit predecessor. Original
/// serialized records are separately archived verbatim for audit/backup fidelity.
pub(super) fn remove_state_numbers(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    use rusqlite::types::Value as SqlValue;
    use std::collections::HashMap;
    type Key = (String, String, i64);
    let mut changes = HashMap::<Key, String>::new();
    {
        let mut statement =
            tx.prepare("SELECT subject_id, memory_id, revision FROM memory_revisions")?;
        for row in statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })? {
            changes.insert(row?, issued_id("memory-change"));
        }
    }
    let mut inputs = HashMap::<(String, i64), String>::new();
    // Old counters cannot establish a complete historical input set. Retain
    // runs/snapshots for audit, but never advertise them as fresh after conversion.
    let mut legacy_input = |subject: &str, number: i64| -> String {
        inputs
            .entry((subject.into(), number))
            .or_insert_with(|| issued_id("archived-input"))
            .clone()
    };
    const TABLES: &[&str] = &[
        "store_scope",
        "subjects",
        "staging_records",
        "memory_records",
        "memory_revisions",
        "memory_revision_candidates",
        "memory_revision_derivations",
        "staging_resolutions",
        "staging_resolution_targets",
        "surface_snapshots",
        "surface_snapshot_refs",
        "memory_revision_seals",
        "staging_resolution_seals",
        "surface_snapshot_seals",
        "subject_session_attributions",
        "candidate_decision_receipts",
        "surface_generation_state",
        "surface_generation_runs",
    ];
    let mut tables = Vec::new();
    for table in TABLES {
        let mut statement = tx.prepare(&format!("SELECT * FROM {table}"))?;
        let columns = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let rows = statement
            .query_map([], |row| {
                (0..columns.len())
                    .map(|i| row.get::<_, SqlValue>(i))
                    .collect::<std::result::Result<Vec<_>, _>>()
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        tables.push((*table, columns, rows));
    }
    let objects = {
        let mut statement = tx.prepare("SELECT type, name FROM sqlite_master WHERE type IN ('trigger','index') AND sql IS NOT NULL")?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (kind, name) in objects {
        // Feature-storage owns its migration metadata; never rebuild its indexes.
        if !name.starts_with("__yoi_") {
            tx.execute_batch(&format!("DROP {kind} \"{name}\";"))?;
        }
    }
    // Deferring all constraints allows the cyclic current/history pair to be
    // rebuilt in this transaction. Verify the full replacement before commit.
    tx.execute_batch("PRAGMA defer_foreign_keys = ON;")?;
    for table in TABLES.iter().rev() {
        tx.execute_batch(&format!("DROP TABLE {table};"))?;
    }
    schema::create(tx)?;
    tx.execute_batch("CREATE TABLE legacy_record_archive (source_table TEXT NOT NULL, row_position INTEGER NOT NULL, row_json TEXT NOT NULL, PRIMARY KEY(source_table, row_position));")?;
    for (table, columns, rows) in tables {
        for (position, values) in rows.into_iter().enumerate() {
            let archive = columns
                .iter()
                .zip(&values)
                .map(|(column, value)| {
                    let value = match value {
                        SqlValue::Null => serde_json::Value::Null,
                        SqlValue::Integer(value) => serde_json::json!(value),
                        SqlValue::Real(value) => serde_json::json!(value),
                        SqlValue::Text(value) => serde_json::json!(value),
                        SqlValue::Blob(value) => serde_json::json!(value),
                    };
                    (column.clone(), value)
                })
                .collect::<serde_json::Map<_, _>>();
            tx.execute(
                "INSERT INTO legacy_record_archive VALUES (?1, ?2, ?3)",
                params![
                    table,
                    position as i64,
                    serde_json::Value::Object(archive).to_string()
                ],
            )?;
            let text = |column: &str| -> String {
                columns
                    .iter()
                    .position(|name| name == column)
                    .and_then(|i| match &values[i] {
                        SqlValue::Text(value) => Some(value.clone()),
                        _ => None,
                    })
                    .unwrap_or_default()
            };
            let subject = text("subject_id");
            let memory = text("memory_id");
            let mut output_columns = Vec::new();
            let mut output_values = Vec::new();
            for (column, value) in columns.iter().zip(&values) {
                if table == "subjects"
                    && matches!(column.as_str(), "store_revision" | "behavior_revision")
                {
                    continue;
                }
                let (name, value) = match column.as_str() {
                    "revision" | "current_revision" | "source_revision" => {
                        let number = match value {
                            SqlValue::Integer(value) => *value,
                            _ => {
                                return Err(FeatureStorageError::operation(
                                    "invalid old Memory identity",
                                ));
                            }
                        };
                        let target = if column == "source_revision" {
                            text("source_memory_id")
                        } else {
                            memory.clone()
                        };
                        let id = changes
                            .get(&(subject.clone(), target, number))
                            .ok_or_else(|| {
                                FeatureStorageError::operation("dangling old Memory reference")
                            })?
                            .clone();
                        let name = match column.as_str() {
                            "current_revision" => "current_change_id",
                            "source_revision" => "source_change_id",
                            _ => "change_id",
                        };
                        (name.to_string(), SqlValue::Text(id))
                    }
                    "store_revision" | "built_from_store_revision" => {
                        let number = match value {
                            SqlValue::Integer(value) => *value,
                            _ => {
                                return Err(FeatureStorageError::operation(
                                    "invalid old input identity",
                                ));
                            }
                        };
                        let name = if column == "store_revision" {
                            "memory_fingerprint"
                        } else {
                            "built_from_memory_fingerprint"
                        };
                        (
                            name.to_string(),
                            SqlValue::Text(legacy_input(&subject, number)),
                        )
                    }
                    "record_json" | "affected_refs_json" | "staging_raw_json"
                    | "resolution_json" | "snapshot_json" | "request_json" | "receipt_json"
                    | "materials_json" => {
                        let raw = match value {
                            SqlValue::Text(value) => value,
                            _ => {
                                return Err(FeatureStorageError::operation(
                                    "invalid old JSON record",
                                ));
                            }
                        };
                        let mut json: serde_json::Value = serde_json::from_str(raw)
                            .map_err(|e| FeatureStorageError::operation(e.to_string()))?;
                        convert_json(&mut json, &subject, &memory, &changes, &mut legacy_input)?;
                        if table == "memory_records" || table == "memory_revisions" {
                            let old_number = match &values[columns
                                .iter()
                                .position(|name| {
                                    name == if table == "memory_records" {
                                        "current_revision"
                                    } else {
                                        "revision"
                                    }
                                })
                                .unwrap()]
                            {
                                SqlValue::Integer(value) => *value,
                                _ => unreachable!(),
                            };
                            let previous = if old_number == 1 {
                                None
                            } else {
                                Some(
                                    changes
                                        .get(&(subject.clone(), memory.clone(), old_number - 1))
                                        .ok_or_else(|| {
                                            FeatureStorageError::operation(
                                                "broken old Memory history",
                                            )
                                        })?
                                        .clone(),
                                )
                            };
                            json["previous_change_id"] = serde_json::json!(previous);
                        }
                        // Preserve canonical request bytes for exact decision replay.
                        let raw = if column == "request_json" {
                            let request: CandidateDecisionRequest = serde_json::from_value(json)
                                .map_err(|e| FeatureStorageError::operation(e.to_string()))?;
                            serde_json::to_string(&request)
                                .map_err(|e| FeatureStorageError::operation(e.to_string()))?
                        } else {
                            json.to_string()
                        };
                        (column.clone(), SqlValue::Text(raw))
                    }
                    "status" if table == "surface_generation_state" => {
                        (column.clone(), SqlValue::Text("dirty".into()))
                    }
                    "snapshot_id" if table == "surface_generation_state" => {
                        (column.clone(), SqlValue::Null)
                    }
                    "reason_code" if table == "surface_generation_state" => (
                        column.clone(),
                        SqlValue::Text("legacy_inputs_require_regeneration".into()),
                    ),
                    _ => (column.clone(), value.clone()),
                };
                output_columns.push(name);
                output_values.push(value);
            }
            if table == "memory_revisions" {
                let number =
                    match &values[columns.iter().position(|name| name == "revision").unwrap()] {
                        SqlValue::Integer(value) => *value,
                        _ => unreachable!(),
                    };
                output_columns.push("previous_change_id".into());
                output_values.push(if number == 1 {
                    SqlValue::Null
                } else {
                    SqlValue::Text(changes[&(subject.clone(), memory.clone(), number - 1)].clone())
                });
            }
            let target_table = table
                .replace("memory_revisions", "memory_changes")
                .replace("memory_revision", "memory_change");
            let placeholders = (1..=output_values.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            tx.execute(
                &format!(
                    "INSERT INTO {target_table} ({}) VALUES ({placeholders})",
                    output_columns.join(",")
                ),
                rusqlite::params_from_iter(output_values),
            )?;
        }
    }
    tx.execute_batch("CREATE TRIGGER legacy_record_archive_no_update BEFORE UPDATE ON legacy_record_archive BEGIN SELECT RAISE(ABORT, 'legacy records are immutable'); END; CREATE TRIGGER legacy_record_archive_no_delete BEFORE DELETE ON legacy_record_archive BEGIN SELECT RAISE(ABORT, 'legacy records are retained'); END;")?;
    let violation = tx
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some();
    if violation {
        return Err(FeatureStorageError::operation(
            "converted Memory references violate foreign keys",
        ));
    }
    Ok(())
}

fn convert_json(
    value: &mut serde_json::Value,
    subject: &str,
    memory: &str,
    changes: &std::collections::HashMap<(String, String, i64), String>,
    legacy_input: &mut impl FnMut(&str, i64) -> String,
) -> feature_storage::Result<()> {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                convert_json(value, subject, memory, changes, legacy_input)?;
            }
        }
        serde_json::Value::Object(object) => {
            let subject = object
                .get("subject_id")
                .and_then(|v| v.as_str())
                .unwrap_or(subject)
                .to_string();
            let memory = object
                .get("memory_id")
                .or_else(|| {
                    if object.contains_key("revision") && object.contains_key("body_md") {
                        object.get("id")
                    } else {
                        None
                    }
                })
                .and_then(|v| v.as_str())
                .unwrap_or(memory)
                .to_string();
            object.remove("behavior_revision");
            // Evidence provenance keeps the exact old origin in the archive. A
            // counter does not reveal its definition content, so do not invent it.
            object.remove("flow_definition_revision");
            for (old, new) in [
                ("revision", "change_id"),
                ("expected_revision", "expected_change_id"),
            ] {
                if let Some(number) = object.remove(old) {
                    let number = number.as_i64().ok_or_else(|| {
                        FeatureStorageError::operation("invalid old Memory reference")
                    })?;
                    let id = changes
                        .get(&(subject.clone(), memory.clone(), number))
                        .ok_or_else(|| {
                            FeatureStorageError::operation(
                                "old JSON has a dangling Memory reference",
                            )
                        })?
                        .clone();
                    object.insert(new.into(), serde_json::json!(id));
                    if old == "revision"
                        && object.contains_key("body_md")
                        && object.contains_key("state")
                    {
                        let previous = if number == 1 {
                            None
                        } else {
                            Some(
                                changes
                                    .get(&(subject.clone(), memory.clone(), number - 1))
                                    .ok_or_else(|| {
                                        FeatureStorageError::operation("broken old JSON history")
                                    })?
                                    .clone(),
                            )
                        };
                        object.insert("previous_change_id".into(), serde_json::json!(previous));
                    }
                }
            }
            if let Some(proposal) = object.remove("revision_proposal") {
                object.insert("change_proposal".into(), proposal);
            }
            for (old, new) in [
                ("store_revision", "memory_fingerprint"),
                ("built_from_store_revision", "built_from_memory_fingerprint"),
            ] {
                if let Some(number) = object.remove(old) {
                    if object.contains_key("role") {
                        continue;
                    }
                    let number = number.as_i64().ok_or_else(|| {
                        FeatureStorageError::operation("invalid old input reference")
                    })?;
                    object.insert(
                        new.into(),
                        serde_json::json!(legacy_input(&subject, number)),
                    );
                }
            }
            for value in object.values_mut() {
                convert_json(value, &subject, &memory, changes, legacy_input)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{attribution, candidate, open_store, role};
    use feature_storage::FeatureStorage;
    #[derive(Serialize)]
    struct LegacySnapshot {
        schema_version: u32,
        id: String,
        subject_id: String,
        body_md: String,
        memory_refs: Vec<MemoryChangeRef>,
        #[serde(rename = "built_from_store_revision")]
        legacy_memory_counter: u64,
        created_at: String,
    }
    static V1_MIGRATIONS: &[FeatureMigration] = &[FeatureMigration::new(
        1,
        "create subjektiv subject memory store",
        create_schema,
    )];
    static V3_MIGRATIONS: &[FeatureMigration] = &[
        FeatureMigration::new(1, "create subjektiv subject memory store", create_schema),
        FeatureMigration::new(
            2,
            "add immutable subject session attribution",
            add_subject_session_attribution,
        ),
        FeatureMigration::new(
            3,
            "add idempotent atomic candidate decision receipts",
            add_candidate_decision_receipts,
        ),
    ];

    #[test]
    fn v1_store_migrates_before_attributed_staging() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        let subject_id = {
            let manager = FeatureStorage::new(&root);
            let workspace = manager.workspace("workspace-a").unwrap();
            let registration = workspace
                .register(FeatureRegistration::new(
                    SUBJEKTIV_FEATURE_ID,
                    V1_MIGRATIONS,
                ))
                .unwrap();
            let store = SubjektivStore::open(&workspace, &registration).unwrap();
            let timestamp = now();
            let subject_id = issued_id("subject");
            let mut raw = serde_json::json!({
                "schema_version": SUBJEKTIV_SCHEMA_VERSION,
                "id": subject_id,
                "role": role(),
                "state": SubjectState::Active,
                "store_revision": 0,
                "created_at": timestamp.clone(),
                "updated_at": timestamp.clone(),
            });
            let raw = serde_json::to_string(&raw.take()).unwrap();
            store
                .database
                .try_transaction(|transaction| -> Result<()> {
                    transaction.execute(
                        "INSERT INTO subjects (
                            subject_id, role, state, store_revision,
                            record_json, created_at, updated_at
                         ) VALUES (?1, ?2, 'active', 0, ?3, ?4, ?5)",
                        params![subject_id, role().as_str(), raw, timestamp, timestamp],
                    )?;
                    Ok(())
                })
                .unwrap();
            subject_id
        };

        let (_manager, _workspace, store) = open_store(&root, "workspace-a");
        let migrated_subject = store.subject(&subject_id).unwrap().unwrap();
        assert!(migrated_subject.behavior_md.is_empty());
        assert!(migrated_subject.behavior_md.is_empty());
        let lifecycle_attribution = attribution(
            &subject_id,
            "runtime-1",
            "worker-1",
            "session-1",
            "2026-09-28T10:00:00.000Z",
        );
        store
            .record_session_attribution(lifecycle_attribution.clone())
            .unwrap();
        let (staged, stored_attribution) = store
            .stage_candidate_with_attribution(
                candidate(&subject_id, "candidate-after-migration", "workspace-a"),
                lifecycle_attribution,
            )
            .unwrap();

        assert_eq!(staged.subject_id, subject_id);
        assert_eq!(stored_attribution.session_id, "session-1");
        assert_eq!(
            store.session_attribution("session-1").unwrap().unwrap(),
            stored_attribution
        );
    }

    #[test]
    fn legacy_surface_snapshot_migrates_as_stale_until_regenerated() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        let (subject_id, snapshot_id) = {
            let manager = FeatureStorage::new(&root);
            let workspace = manager.workspace("workspace-a").unwrap();
            let registration = workspace
                .register(FeatureRegistration::new(
                    SUBJEKTIV_FEATURE_ID,
                    V3_MIGRATIONS,
                ))
                .unwrap();
            let store = SubjektivStore::open(&workspace, &registration).unwrap();
            let subject_id = issued_id("subject");
            let timestamp = now();
            let subject_json = serde_json::to_string(&serde_json::json!({
                "schema_version": SUBJEKTIV_SCHEMA_VERSION,
                "id": subject_id,
                "role": role(),
                "state": SubjectState::Active,
                "store_revision": 0,
                "created_at": timestamp,
                "updated_at": timestamp,
            }))
            .unwrap();
            let snapshot = LegacySnapshot {
                schema_version: SUBJEKTIV_SCHEMA_VERSION,
                id: "legacy-surface".into(),
                subject_id: subject_id.clone(),
                body_md: "Legacy summary without generation-policy evidence.".into(),
                memory_refs: Vec::new(),
                legacy_memory_counter: 0,
                created_at: now(),
            };
            store
                .database
                .try_transaction(|transaction| -> Result<()> {
                    transaction.execute(
                        "INSERT INTO subjects (
                            subject_id, role, state, store_revision,
                            record_json, created_at, updated_at
                         ) VALUES (?1, ?2, 'active', 0, ?3, ?4, ?5)",
                        params![
                            subject_id,
                            role().as_str(),
                            subject_json,
                            timestamp,
                            timestamp
                        ],
                    )?;
                    transaction.execute(
                        "INSERT INTO surface_snapshots (
                            subject_id, snapshot_id, built_from_store_revision,
                            snapshot_json, created_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            subject_id,
                            snapshot.id,
                            to_i64(snapshot.legacy_memory_counter)?,
                            serde_json::to_string(&snapshot)?,
                            snapshot.created_at
                        ],
                    )?;
                    transaction.execute(
                        "INSERT INTO surface_snapshot_seals (subject_id, snapshot_id)
                         VALUES (?1, ?2)",
                        params![subject_id, snapshot.id],
                    )?;
                    Ok(())
                })
                .unwrap();
            (subject_id, snapshot.id)
        };

        let (_manager, _workspace, store) = open_store(&root, "workspace-a");
        let resident = store.resident_surface(&subject_id).unwrap();
        assert_eq!(resident.availability, SurfaceAvailability::Stale);
        assert!(resident.snapshot.is_none());
        assert!(
            store
                .surface_snapshot(&subject_id, &snapshot_id)
                .unwrap()
                .is_some(),
            "migration retains the legacy snapshot as history without injecting it"
        );

        let generation = store.prepare_surface_generation(&subject_id).unwrap();
        let regenerated = store
            .publish_surface_generation(&subject_id, &generation.id, Vec::new())
            .unwrap();
        assert_ne!(regenerated.id, snapshot_id);
        let ready = store.resident_surface(&subject_id).unwrap();
        assert_eq!(ready.availability, SurfaceAvailability::Ready);
        assert_eq!(ready.snapshot.unwrap().id, regenerated.id);
        assert!(
            store
                .surface_snapshot(&subject_id, &snapshot_id)
                .unwrap()
                .is_some(),
            "regeneration appends instead of mutating legacy history"
        );
        let retried = store
            .publish_surface_generation(&subject_id, &generation.id, Vec::new())
            .unwrap();
        assert_eq!(retried.id, regenerated.id);
    }
}

#[cfg(test)]
mod conversion_tests {
    use super::*;
    use crate::tests::{open_store, role};
    use feature_storage::FeatureStorage;
    use serde_json::json;
    static V6: &[FeatureMigration] = &[
        FeatureMigration::new(1, "create subjektiv subject memory store", create_schema),
        FeatureMigration::new(
            2,
            "add immutable subject session attribution",
            add_subject_session_attribution,
        ),
        FeatureMigration::new(
            3,
            "add idempotent atomic candidate decision receipts",
            add_candidate_decision_receipts,
        ),
        FeatureMigration::new(
            4,
            "add bounded surface generation state and runs",
            add_surface_generation_state,
        ),
        FeatureMigration::new(5, "add user-managed subject behavior", add_subject_behavior),
        FeatureMigration::new(
            6,
            "bind surface generation provenance to Job attempts",
            add_surface_job_provenance,
        ),
    ];

    #[test]
    fn populated_v6_conversion_preserves_history_edges_receipts_and_raw_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let at = "2026-10-01T10:00:00.000Z";
        let subject = json!({"schema_version":1,"id":"subject","role":role(),"behavior_md":"Preserve me.","behavior_revision":7,"state":"active","store_revision":2,"created_at":at,"updated_at":at});
        let candidate = json!({"schema_version":2,"id":"candidate","subject_id":"subject","extract_run_id":"extract","source":{"segment_id":"segment","range":[1,1]},"kind":"constraint","claim":"original","why_useful":"useful","created_at":at,"evidence":[{"id":"e","kind":"message","origin":{"kind":"flow_instruction","flow_definition_id":"flow","flow_definition_revision":9}}],"source_refs":[]});
        let first = json!({"schema_version":1,"id":"memory","subject_id":"subject","revision":1,"kind":"constraint","state":"active","claim":"original","body_md":"original body","why_useful":"useful","source_candidate_ids":["candidate"],"derived_from":[],"change_reason":"accepted","created_at":at,"updated_at":at});
        let mut second = first.clone();
        second["revision"] = json!(2);
        second["claim"] = json!("corrected");
        second["body_md"] = json!("corrected body");
        second["source_candidate_ids"] = json!([]);
        second["derived_from"] = json!([{"memory_id":"memory","revision":1}]);
        let affected = json!([{"memory_id":"memory","revision":1}]);
        let resolution = json!({"schema_version":1,"id":"resolution","subject_id":"subject","candidate_id":"candidate","action":"applied","reason":"accepted","affected_memory":affected,"candidate":candidate,"resolved_at":at});
        let request = json!({"request_id":"decision","candidate_id":"candidate","reason":"accepted","decision":{"kind":"apply","target":{"kind":"create"},"draft":{"kind":"constraint","state":"active","claim":"original","body_md":"original body","why_useful":"useful","staleness":null,"source_candidate_ids":[],"derived_from":[],"change_reason":"accepted"}}});
        let receipt = json!({"request_id":"decision","candidate_id":"candidate","resolution":resolution,"memory":first,"operation":"create","store_revision":1,"surface_dirty":true});
        let snapshot = json!({"schema_version":1,"id":"snapshot","subject_id":"subject","body_md":"summary","memory_refs":[{"memory_id":"memory","revision":2}],"built_from_store_revision":2,"created_at":at});
        let materials = json!([{"memory_id":"memory","revision":2,"kind":"constraint","body_md":"corrected body","why_useful":"useful","staleness":null}]);
        let original_candidate_bytes = format!(" {}\n", candidate);
        {
            let manager = FeatureStorage::new(temp.path());
            let scope = manager.scope("scope").unwrap();
            let registration = scope
                .register(FeatureRegistration::new(SUBJEKTIV_FEATURE_ID, V6))
                .unwrap();
            let store = SubjektivStore::open(&scope, &registration).unwrap();
            store.database.try_transaction::<_, SubjektivError>(|tx| {
                tx.execute("INSERT INTO subjects (subject_id,role,state,store_revision,record_json,created_at,updated_at,behavior_md,behavior_revision) VALUES ('subject',?1,'active',2,?2,?3,?3,'Preserve me.',7)",params![role().as_str(),subject.to_string(),at])?;
                tx.execute("INSERT INTO staging_records VALUES ('subject','candidate','constraint',?1,?2)",params![original_candidate_bytes,at])?;
                tx.execute("INSERT INTO memory_records VALUES ('subject','memory',2,'constraint','active',?1,?2,?2)",params![second.to_string(),at])?;
                for (number, record) in [(1,&first),(2,&second)] {
                    tx.execute("INSERT INTO memory_revisions VALUES ('subject','memory',?1,'constraint','active',?2,?3)",params![number,record.to_string(),at])?;
                }
                tx.execute("INSERT INTO memory_revision_candidates VALUES ('subject','memory',1,'candidate')",[])?;
                tx.execute("INSERT INTO memory_revision_derivations VALUES ('subject','memory',2,'memory',1)",[])?;
                tx.execute("INSERT INTO staging_resolutions VALUES ('subject','candidate','resolution','applied','accepted',?1,?2,?3,?4)",params![affected.to_string(),original_candidate_bytes,resolution.to_string(),at])?;
                tx.execute("INSERT INTO staging_resolution_targets VALUES ('subject','candidate','memory',1)",[])?;
                tx.execute("INSERT INTO surface_snapshots VALUES ('subject','snapshot',2,?1,?2)",params![snapshot.to_string(),at])?;
                tx.execute("INSERT INTO surface_snapshot_refs VALUES ('subject','snapshot','memory',2)",[])?;
                tx.execute("INSERT INTO memory_revision_seals VALUES ('subject','memory',1),('subject','memory',2)",[])?;
                tx.execute("INSERT INTO staging_resolution_seals VALUES ('subject','candidate')",[])?;
                tx.execute("INSERT INTO surface_snapshot_seals VALUES ('subject','snapshot')",[])?;
                tx.execute("INSERT INTO candidate_decision_receipts VALUES ('subject','decision','candidate',?1,?2,?3)",params![request.to_string(),receipt.to_string(),at])?;
                tx.execute("INSERT INTO surface_generation_runs VALUES ('subject','generation',2,1,?1,?2,'job','attempt')",params![materials.to_string(),at])?;
                tx.execute("INSERT INTO surface_generation_state VALUES ('subject',2,'ready','snapshot',NULL,?1)",[at])?;
                Ok(())
            }).unwrap();
        }
        let (_manager, _scope, store) = open_store(temp.path(), "scope");
        assert_eq!(
            store.subject("subject").unwrap().unwrap().behavior_md,
            "Preserve me."
        );
        let history = store.list_memory_changes("subject", "memory").unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].body_md, "corrected body");
        assert_eq!(history[1].body_md, "original body");
        assert_eq!(
            history[0].previous_change_id.as_deref(),
            Some(history[1].change_id.as_str())
        );
        assert_eq!(history[0].derived_from[0].change_id, history[1].change_id);
        assert_eq!(history[1].source_candidate_ids, ["candidate"]);
        let staged = store
            .staging_candidate("subject", "candidate")
            .unwrap()
            .unwrap();
        assert_eq!(
            staged.evidence[0]
                .origin
                .as_ref()
                .unwrap()
                .flow_definition_id
                .as_deref(),
            Some("flow")
        );
        assert_eq!(
            staged.evidence[0]
                .origin
                .as_ref()
                .unwrap()
                .flow_definition_fingerprint,
            None
        );
        assert_eq!(
            store
                .surface_snapshot("subject", "snapshot")
                .unwrap()
                .unwrap()
                .memory_refs[0]
                .change_id,
            history[0].change_id
        );
        assert_eq!(
            store.resident_surface("subject").unwrap().availability,
            SurfaceAvailability::Stale
        );
        let request_raw: String = store
            .database
            .try_with_connection::<_, SubjektivError>(|conn| {
                Ok(conn.query_row(
                    "SELECT request_json FROM candidate_decision_receipts",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        let replay = store
            .decide_candidate("subject", serde_json::from_str(&request_raw).unwrap())
            .unwrap();
        assert_eq!(replay.memory.unwrap().change_id, history[1].change_id);
        assert_eq!(
            replay.resolution.affected_memory[0].change_id,
            history[1].change_id
        );
        let archived: String = store.database.try_with_connection::<_, SubjektivError>(|conn| Ok(conn.query_row("SELECT row_json FROM legacy_record_archive WHERE source_table='staging_records'",[],|row|row.get(0))?)).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&archived).unwrap()["record_json"],
            original_candidate_bytes
        );
        let violations: i64 = store
            .database
            .try_with_connection::<_, SubjektivError>(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(violations, 0);
    }
}
