use super::*;

// Frozen pre-cutover schema, including the retired persisted state spelling.
pub(super) const SCHEMA: &str = "
CREATE TABLE job_intents (
    job_id TEXT PRIMARY KEY NOT NULL,
    request_json TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    serialization_key TEXT,
    max_concurrent INTEGER NOT NULL CHECK (max_concurrent BETWEEN 1 AND 65535),
    state TEXT NOT NULL CHECK (state IN ('pending', 'completed', 'failed', 'unknown', 'cancelled')),
    current_attempt INTEGER NOT NULL CHECK (current_attempt BETWEEN 1 AND 255),
    acknowledged INTEGER NOT NULL DEFAULT 0 CHECK (acknowledged IN (0, 1)),
    CHECK (acknowledged = 0 OR state = 'completed'),
    FOREIGN KEY (job_id, current_attempt) REFERENCES job_attempts(job_id, number)
        DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE job_attempts (
    job_id TEXT NOT NULL REFERENCES job_intents(job_id),
    number INTEGER NOT NULL CHECK (number BETWEEN 1 AND 255),
    attempt_id TEXT NOT NULL UNIQUE,
    input_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('reserved', 'dispatched', 'completed', 'failed', 'unknown', 'cancelled')),
    result_json TEXT,
    result_digest TEXT,
    failure TEXT,
    PRIMARY KEY (job_id, number),
    CHECK ((result_json IS NULL) = (result_digest IS NULL)),
    CHECK (state <> 'completed' OR result_json IS NOT NULL)
);
CREATE UNIQUE INDEX job_serialization_singleflight ON job_intents(serialization_key)
    WHERE serialization_key IS NOT NULL AND state IN ('pending', 'unknown');
CREATE INDEX job_attempt_state ON job_attempts(state);
CREATE TRIGGER job_intent_immutable BEFORE UPDATE OF
    job_id, request_json, fingerprint, serialization_key, max_concurrent ON job_intents
BEGIN SELECT RAISE(ABORT, 'immutable Job intent'); END;
CREATE TRIGGER job_attempt_immutable BEFORE UPDATE OF
    job_id, number, attempt_id, input_digest ON job_attempts
BEGIN SELECT RAISE(ABORT, 'immutable Job attempt identity'); END;
CREATE TRIGGER job_result_immutable BEFORE UPDATE OF result_json, result_digest ON job_attempts
WHEN OLD.result_json IS NOT NULL AND
    (NEW.result_json IS NOT OLD.result_json OR NEW.result_digest IS NOT OLD.result_digest)
BEGIN SELECT RAISE(ABORT, 'immutable accepted Job result'); END;
";

/// Frozen schema-1/2 adapter. Preserve original intent and attempt bindings
/// before replacing counters with identities derived from immutable input.
pub(super) fn migrate_legacy_jobs(conn: &Connection) -> Result<(), JobStoreError> {
    conn.execute_batch(
        "CREATE TABLE legacy_job_archive (
            job_id TEXT PRIMARY KEY NOT NULL,
            request_json TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            attempts_json TEXT NOT NULL
        );
        DROP TRIGGER job_intent_immutable;
        DROP TRIGGER job_attempt_immutable;",
    )?;
    let intents = conn
        .prepare("SELECT job_id, request_json, fingerprint FROM job_intents")?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, original, fingerprint) in intents {
        let attempts = conn.prepare(
            "SELECT number, attempt_id, input_revision, state, result_json, result_digest, failure
             FROM job_attempts WHERE job_id = ?1 ORDER BY number",
        )?.query_map([&id], |r| Ok(serde_json::json!({
            "number":r.get::<_, u8>(0)?, "attempt_id":r.get::<_, String>(1)?,
            "input_revision":r.get::<_, String>(2)?, "state":r.get::<_, String>(3)?,
            "result_json":r.get::<_, Option<String>>(4)?, "result_digest":r.get::<_, Option<String>>(5)?,
            "failure":r.get::<_, Option<String>>(6)?
        })))?.collect::<Result<Vec<_>, _>>()?;
        conn.execute(
            "INSERT INTO legacy_job_archive VALUES (?1, ?2, ?3, ?4)",
            params![id, original, fingerprint, serde_json::to_string(&attempts)?],
        )?;
        let mut value: Value = serde_json::from_str(&original)?;
        value
            .as_object_mut()
            .ok_or_else(|| JobStoreError::Corrupt("legacy request is not an object".into()))?
            .remove("input_revision");
        let request: JobRequest = serde_json::from_value(value)?;
        if request.job_id != id {
            return Err(JobStoreError::Corrupt(
                "legacy request Job id mismatch".into(),
            ));
        }
        conn.execute(
            "UPDATE job_intents SET request_json = ?2, fingerprint = ?3 WHERE job_id = ?1",
            params![id, serde_json::to_string(&request)?, request.fingerprint()?],
        )?;
        conn.execute(
            "UPDATE job_attempts SET input_revision = ?2 WHERE job_id = ?1",
            params![id, request.input_digest()?],
        )?;
    }
    conn.execute_batch(
        "ALTER TABLE job_attempts RENAME COLUMN input_revision TO input_digest;
        CREATE TRIGGER job_intent_immutable BEFORE UPDATE OF
            job_id, request_json, fingerprint, serialization_key, max_concurrent ON job_intents
        BEGIN SELECT RAISE(ABORT, 'immutable Job intent'); END;
        CREATE TRIGGER job_attempt_immutable BEFORE UPDATE OF
            job_id, number, attempt_id, input_digest ON job_attempts
        BEGIN SELECT RAISE(ABORT, 'immutable Job attempt identity'); END;
        CREATE TRIGGER legacy_job_archive_immutable BEFORE UPDATE ON legacy_job_archive
        BEGIN SELECT RAISE(ABORT, 'immutable legacy Job archive'); END;
        CREATE TRIGGER legacy_job_archive_retained BEFORE DELETE ON legacy_job_archive
        BEGIN SELECT RAISE(ABORT, 'retained legacy Job archive'); END;",
    )?;
    Ok(())
}

// Frozen schema fixtures for every supported storage boundary.
pub(super) const RECOVERY_SCHEMA: &str = "
CREATE TABLE job_version_history (
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL PRIMARY KEY,
    job_state TEXT NOT NULL,
    attempt_state TEXT NOT NULL,
    failure TEXT
);
CREATE TRIGGER job_version_history_immutable BEFORE UPDATE ON job_version_history
BEGIN SELECT RAISE(ABORT,'immutable previous Job state'); END;
CREATE TRIGGER job_version_history_retained BEFORE DELETE ON job_version_history
BEGIN SELECT RAISE(ABORT,'retained previous Job state'); END;
CREATE TABLE job_version_stops (job_id TEXT NOT NULL PRIMARY KEY);
CREATE TRIGGER job_version_stop_immutable BEFORE UPDATE ON job_version_stops
BEGIN SELECT RAISE(ABORT,'immutable version stop'); END;
CREATE TRIGGER job_version_stop_retained BEFORE DELETE ON job_version_stops
BEGIN SELECT RAISE(ABORT,'retained version stop'); END;
CREATE TABLE job_execution_ownership (
    job_id TEXT NOT NULL REFERENCES job_intents(job_id),
    attempt_id TEXT NOT NULL PRIMARY KEY REFERENCES job_attempts(attempt_id),
    owner_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    writes_revoked INTEGER NOT NULL DEFAULT 0 CHECK(writes_revoked IN (0,1)),
    cleanup_receipt TEXT,
    CHECK(cleanup_receipt IS NULL OR writes_revoked=1)
);
CREATE TRIGGER job_execution_identity_immutable BEFORE UPDATE OF job_id,attempt_id,owner_id,run_id ON job_execution_ownership
BEGIN SELECT RAISE(ABORT,'immutable Job execution binding'); END;
CREATE TRIGGER job_execution_proof_monotonic BEFORE UPDATE ON job_execution_ownership
WHEN NEW.writes_revoked < OLD.writes_revoked
 OR (OLD.cleanup_receipt IS NOT NULL AND NEW.cleanup_receipt IS NOT OLD.cleanup_receipt)
BEGIN SELECT RAISE(ABORT,'immutable execution proof'); END;
CREATE TRIGGER job_execution_retained BEFORE DELETE ON job_execution_ownership
BEGIN SELECT RAISE(ABORT,'retained Job execution'); END;
";

pub(super) const START_REJECTION_SCHEMA: &str = "
CREATE TABLE job_start_rejections (
    job_id TEXT NOT NULL REFERENCES job_intents(job_id),
    attempt_id TEXT NOT NULL PRIMARY KEY REFERENCES job_attempts(attempt_id),
    owner_id TEXT NOT NULL
);
CREATE TRIGGER job_start_rejection_guard BEFORE INSERT ON job_start_rejections
WHEN NOT EXISTS(SELECT 1 FROM job_intents i JOIN job_attempts a ON a.job_id=i.job_id AND a.number=i.current_attempt
    WHERE i.job_id=NEW.job_id AND a.attempt_id=NEW.attempt_id AND i.state='pending' AND a.state='reserved')
 OR EXISTS(SELECT 1 FROM job_execution_ownership WHERE attempt_id=NEW.attempt_id)
 OR EXISTS(SELECT 1 FROM job_domain_grants WHERE job_id=NEW.job_id)
 OR EXISTS(SELECT 1 FROM job_version_stops WHERE job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'start rejection requires unstarted result-only reservation'); END;
CREATE TRIGGER job_start_rejection_immutable BEFORE UPDATE ON job_start_rejections
BEGIN SELECT RAISE(ABORT,'immutable Job start rejection'); END;
CREATE TRIGGER job_start_rejection_retained BEFORE DELETE ON job_start_rejections
BEGIN SELECT RAISE(ABORT,'retained Job start rejection'); END;
";

pub(super) const GRANTED_START_REJECTION_SCHEMA: &str = "
DROP TRIGGER job_start_rejection_guard;
CREATE TRIGGER job_start_rejection_guard BEFORE INSERT ON job_start_rejections
WHEN NOT EXISTS(SELECT 1 FROM job_intents i JOIN job_attempts a ON a.job_id=i.job_id AND a.number=i.current_attempt
    WHERE i.job_id=NEW.job_id AND a.attempt_id=NEW.attempt_id AND i.state='pending' AND a.state='reserved'
      AND i.input_disposition='available')
 OR EXISTS(SELECT 1 FROM job_execution_ownership WHERE attempt_id=NEW.attempt_id)
 OR EXISTS(SELECT 1 FROM job_execution_ownership WHERE job_id=NEW.job_id AND (writes_revoked=0 OR cleanup_receipt IS NULL))
 OR EXISTS(SELECT 1 FROM job_version_stops WHERE job_id=NEW.job_id)
BEGIN SELECT RAISE(ABORT,'start rejection requires an available unstarted reservation and prior cleanup'); END;
";

pub(super) const SCHEMA_SIX: &str = "
CREATE TABLE job_intents (
    job_id TEXT PRIMARY KEY NOT NULL,
    request_json TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    serialization_key TEXT,
    max_concurrent INTEGER NOT NULL CHECK (max_concurrent BETWEEN 1 AND 65535),
    state TEXT NOT NULL CHECK (state IN ('pending', 'completed', 'failed', 'cancelled')),
    current_attempt INTEGER NOT NULL CHECK (current_attempt BETWEEN 1 AND 255),
    input_disposition TEXT NOT NULL DEFAULT 'available'
        CHECK (input_disposition IN ('available','reclaimed','erased','invalidated')),
    acknowledged INTEGER NOT NULL DEFAULT 0 CHECK (acknowledged IN (0, 1)),
    CHECK (acknowledged = 0 OR state = 'completed'),
    FOREIGN KEY (job_id, current_attempt) REFERENCES job_attempts(job_id, number)
        DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE job_attempts (
    job_id TEXT NOT NULL REFERENCES job_intents(job_id),
    number INTEGER NOT NULL CHECK (number BETWEEN 1 AND 255),
    attempt_id TEXT NOT NULL UNIQUE,
    input_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('reserved', 'dispatched', 'completed', 'failed', 'cancelled')),
    result_json TEXT,
    result_digest TEXT,
    failure TEXT,
    PRIMARY KEY (job_id, number),
    CHECK ((result_json IS NULL) = (result_digest IS NULL)),
    CHECK (state <> 'completed' OR result_json IS NOT NULL)
);
CREATE UNIQUE INDEX job_serialization_singleflight ON job_intents(serialization_key)
    WHERE serialization_key IS NOT NULL AND state = 'pending';
CREATE INDEX job_attempt_state ON job_attempts(state);
CREATE TRIGGER job_intent_immutable BEFORE UPDATE OF
    job_id, fingerprint, serialization_key, max_concurrent ON job_intents
BEGIN SELECT RAISE(ABORT, 'immutable Job intent'); END;
CREATE TRIGGER job_attempt_immutable BEFORE UPDATE OF
    job_id, number, attempt_id, input_digest ON job_attempts
BEGIN SELECT RAISE(ABORT, 'immutable Job attempt identity'); END;
CREATE TRIGGER job_result_immutable BEFORE UPDATE OF result_json, result_digest ON job_attempts
WHEN OLD.result_json IS NOT NULL AND
    (NEW.result_json IS NOT OLD.result_json OR NEW.result_digest IS NOT OLD.result_digest)
BEGIN SELECT RAISE(ABORT, 'immutable accepted Job result'); END;
CREATE TRIGGER job_input_retirement BEFORE UPDATE OF request_json, input_disposition ON job_intents
WHEN (
    ((OLD.input_disposition = 'available' AND NEW.input_disposition IN ('reclaimed','erased','invalidated'))
      OR (OLD.input_disposition IN ('reclaimed','invalidated') AND NEW.input_disposition = 'erased'))
    AND json_remove(NEW.request_json,'$.input') = json_remove(OLD.request_json,'$.input')
    AND json_type(NEW.request_json,'$.input') = 'null'
) IS NOT TRUE
BEGIN SELECT RAISE(ABORT, 'immutable Job input retirement'); END;
";

pub(super) const INPUT_RETENTION_SCHEMA: &str = "
ALTER TABLE job_intents ADD COLUMN input_disposition TEXT NOT NULL DEFAULT 'available'
    CHECK (input_disposition IN ('available','reclaimed','erased','invalidated'));
DROP TRIGGER job_intent_immutable;
CREATE TRIGGER job_intent_immutable BEFORE UPDATE OF
    job_id, fingerprint, serialization_key, max_concurrent ON job_intents
BEGIN SELECT RAISE(ABORT, 'immutable Job intent'); END;
CREATE TRIGGER job_input_retirement BEFORE UPDATE OF request_json, input_disposition ON job_intents
WHEN (
    ((OLD.input_disposition = 'available' AND NEW.input_disposition IN ('reclaimed','erased','invalidated'))
      OR (OLD.input_disposition IN ('reclaimed','invalidated') AND NEW.input_disposition = 'erased'))
    AND json_remove(NEW.request_json,'$.input') = json_remove(OLD.request_json,'$.input')
    AND json_type(NEW.request_json,'$.input') = 'null'
) IS NOT TRUE
BEGIN SELECT RAISE(ABORT, 'immutable Job input retirement'); END;
";
