//! Host-owned durable Jobs. Opening performs recovery and therefore requires the
//! StandaloneHost's Worker lease; this is not a user-facing database constructor.
//!
//! A result is committed here before the service releases its Internal Worker.
//! Worker termination, final prose, and consumer acknowledgement are not success
//! authority. The service owns the connection mutex and live-resource cleanup.

use std::path::Path;

use job::{JobRequest, JobResultSubmission};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

const SCHEMA_VERSION: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Pending,
    Completed,
    Failed,
    Unknown,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobAttemptState {
    Reserved,
    Dispatched,
    Completed,
    Failed,
    Unknown,
    Cancelled,
}

impl JobAttemptState {
    pub fn terminal(self) -> bool {
        !matches!(self, Self::Reserved | Self::Dispatched)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Dispatched => "dispatched",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub request: JobRequest,
    /// Durable Host-issued domain delegation, not model input or Profile identity.
    pub domain_grant: Option<Value>,
    pub state: JobState,
    pub attempt: JobAttempt,
    pub result: Option<Value>,
    pub acknowledged: bool,
}

impl JobSnapshot {
    /// Shared Feature-facing read contract; no Workspace or execution identity leaks.
    pub fn outcome(&self) -> job::JobOutcome {
        job::JobOutcome {
            job_id: self.request.job_id.clone(),
            input_revision: self.request.input_revision.clone(),
            attempt_id: self.attempt.attempt_id.clone(),
            attempt: self.attempt.number,
            state: match self.state {
                JobState::Pending => job::JobState::Pending,
                JobState::Completed => job::JobState::Completed,
                JobState::Failed => job::JobState::Failed,
                JobState::Unknown => job::JobState::Unknown,
                JobState::Cancelled => job::JobState::Cancelled,
            },
            attempt_state: match self.attempt.state {
                JobAttemptState::Reserved => job::JobAttemptState::Reserved,
                JobAttemptState::Dispatched => job::JobAttemptState::Dispatched,
                JobAttemptState::Completed => job::JobAttemptState::Completed,
                JobAttemptState::Failed => job::JobAttemptState::Failed,
                JobAttemptState::Unknown => job::JobAttemptState::Unknown,
                JobAttemptState::Cancelled => job::JobAttemptState::Cancelled,
            },
            result: self.result.clone(),
            failure_category: self.attempt.failure.as_ref().map(|_| {
                match self.attempt.state {
                    JobAttemptState::Cancelled => "cancelled",
                    JobAttemptState::Unknown => "unknown_outcome",
                    _ => "execution_failed",
                }
                .to_string()
            }),
            failure_detail: self.attempt.failure.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobAttempt {
    pub attempt_id: String,
    pub number: u8,
    pub state: JobAttemptState,
    pub failure: Option<String>,
}

#[derive(Debug, Error)]
pub enum JobStoreError {
    #[error("Job database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Storage(#[from] feature_storage::FeatureStorageError),
    #[error("Job JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Job(#[from] job::JobError),
    #[error("Job directory: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported Job schema version {0}")]
    UnsupportedSchema(i64),
    #[error("corrupt Job database: {0}")]
    Corrupt(String),
    #[error("unknown Job `{0}`")]
    NotFound(String),
    #[error("Job id `{0}` was reused with different immutable intent")]
    IntentConflict(String),
    #[error("serialization key `{0}` is reserved by another Job")]
    SerializationConflict(String),
    #[error("Job concurrent execution limit reached")]
    ConcurrentLimit,
    #[error("result or lifecycle operation is not bound to the current Job attempt")]
    AttemptMismatch,
    #[error("result input revision does not match the immutable request")]
    RevisionMismatch,
    #[error("accepted Job result cannot be changed")]
    ResultConflict,
    #[error("invalid Job transition: {0}")]
    InvalidTransition(&'static str),
}

pub(crate) struct JobStore {
    conn: Connection,
}

impl JobStore {
    /// The caller must hold the Host's Worker lease for this connection's entire
    /// lifetime. Recovery must never run beside a live service for this Worker.
    pub(crate) fn open(path: &Path) -> Result<Self, JobStoreError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)?;
        }
        let mut conn = Connection::open(path)?;
        feature_storage::configure_connection(&conn)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => {
                tx.execute_batch(SCHEMA)?;
                tx.execute_batch(GRANT_SCHEMA)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            1 => {
                tx.execute_batch(GRANT_SCHEMA)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            SCHEMA_VERSION => {}
            other => return Err(JobStoreError::UnsupportedSchema(other)),
        }
        // An accepted result is the only evidence of success. Reserved attempts
        // were never started and remain pending; cancelled attempts never resume.
        tx.execute_batch(
            "UPDATE job_attempts SET state = 'completed', failure = NULL
               WHERE state = 'dispatched' AND result_json IS NOT NULL;
             UPDATE job_attempts SET state = 'unknown',
                 failure = 'Host interrupted before an accepted result'
               WHERE state = 'dispatched' AND result_json IS NULL;
             UPDATE job_intents SET state = (
                 SELECT state FROM job_attempts
                 WHERE job_id = job_intents.job_id AND number = current_attempt)
               WHERE state = 'pending' AND EXISTS (
                 SELECT 1 FROM job_attempts WHERE job_id = job_intents.job_id
                 AND number = current_attempt AND state IN ('completed', 'unknown'));",
        )?;
        tx.commit()?;
        Ok(Self { conn })
    }

    pub(crate) fn reserve(&mut self, request: JobRequest) -> Result<JobSnapshot, JobStoreError> {
        self.reserve_granted(request, None)
    }
    pub(crate) fn reserve_granted(
        &mut self,
        request: JobRequest,
        grant: Option<Value>,
    ) -> Result<JobSnapshot, JobStoreError> {
        request.validate()?;
        if let Some(value) = &grant {
            job::result_digest(value, job::ABSOLUTE_MAX_RESULT_BYTES)?;
        }
        let fingerprint = request.fingerprint()?;
        let request_json = serde_json::to_string(&request)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(snapshot) = read_snapshot(&tx, &request.job_id)? {
            let stored: String = tx.query_row(
                "SELECT fingerprint FROM job_intents WHERE job_id = ?1",
                [&request.job_id],
                |row| row.get(0),
            )?;
            if stored != fingerprint
                || snapshot.request != request
                || snapshot.domain_grant != grant
            {
                return Err(JobStoreError::IntentConflict(request.job_id));
            }
            tx.commit()?;
            return Ok(snapshot);
        }
        check_serialization(&tx, request.serialization_key.as_deref(), &request.job_id)?;
        tx.execute(
            "INSERT INTO job_intents
             (job_id, request_json, fingerprint, serialization_key, max_concurrent,
              state, current_attempt, acknowledged)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 1, 0)",
            params![
                request.job_id,
                request_json,
                fingerprint,
                request.serialization_key,
                request.limits.max_concurrent_jobs
            ],
        )?;
        insert_attempt(&tx, &request, 1)?;
        if let Some(grant) = grant {
            tx.execute(
                "INSERT INTO job_domain_grants(job_id, grant_json) VALUES (?1, ?2)",
                params![request.job_id, serde_json::to_string(&grant)?],
            )?;
        }
        let snapshot = require_snapshot(&tx, &request.job_id)?;
        tx.commit()?;
        Ok(snapshot)
    }

    /// Only definitive failure can create a new immutable attempt. Unknown
    /// outcomes require caller reconciliation, never an automatic replay.
    pub(crate) fn retry(&mut self, job_id: &str) -> Result<JobSnapshot, JobStoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = require_snapshot(&tx, job_id)?;
        if previous.state != JobState::Failed || previous.attempt.state != JobAttemptState::Failed {
            return Err(JobStoreError::InvalidTransition(
                "retry requires known failure",
            ));
        }
        if previous.attempt.number >= previous.request.limits.max_attempts {
            return Err(JobStoreError::InvalidTransition("attempt limit reached"));
        }
        check_serialization(&tx, previous.request.serialization_key.as_deref(), job_id)?;
        let next = previous.attempt.number + 1;
        insert_attempt(&tx, &previous.request, next)?;
        tx.execute(
            "UPDATE job_intents SET state = 'pending', current_attempt = ?2 WHERE job_id = ?1",
            params![job_id, next],
        )?;
        let snapshot = require_snapshot(&tx, job_id)?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub(crate) fn get(&self, job_id: &str) -> Result<JobSnapshot, JobStoreError> {
        require_snapshot(&self.conn, job_id)
    }

    pub(crate) fn unacknowledged(&self) -> Result<Vec<JobSnapshot>, JobStoreError> {
        let mut statement = self.conn.prepare("SELECT job_id FROM job_intents WHERE state = 'completed' AND acknowledged = 0 ORDER BY rowid")?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|id| require_snapshot(&self.conn, &id))
            .collect()
    }

    /// Only unstarted current attempts are eligible for service startup/resume.
    pub(crate) fn pending(&self) -> Result<Vec<JobSnapshot>, JobStoreError> {
        let mut statement = self.conn.prepare(
            "SELECT i.job_id FROM job_intents i JOIN job_attempts a
             ON a.job_id = i.job_id AND a.number = i.current_attempt
             WHERE i.state = 'pending' AND a.state = 'reserved' ORDER BY i.rowid",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|id| require_snapshot(&self.conn, &id))
            .collect()
    }

    /// A dispatch is a claim, not an idempotent instruction to spawn again.
    /// Repeating it after the claim is rejected, so one attempt has one Worker.
    pub(crate) fn dispatch(
        &mut self,
        job_id: &str,
        attempt_id: &str,
        max_concurrent: u16,
    ) -> Result<JobSnapshot, JobStoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let snapshot = require_snapshot(&tx, job_id)?;
        check_attempt(&snapshot, attempt_id)?;
        if snapshot.state != JobState::Pending
            || snapshot.attempt.state != JobAttemptState::Reserved
        {
            return Err(JobStoreError::InvalidTransition(
                "dispatch requires a reserved pending attempt",
            ));
        }
        check_serialization(&tx, snapshot.request.serialization_key.as_deref(), job_id)?;
        let (active, active_limit): (i64, Option<u16>) = tx.query_row(
            "SELECT COUNT(*), MIN(i.max_concurrent) FROM job_attempts a JOIN job_intents i
             ON i.job_id = a.job_id AND i.current_attempt = a.number
             WHERE a.state = 'dispatched'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        // Respect the Host ceiling and every currently executing request's
        // ceiling, not just a new request with a more permissive limit.
        let limit = max_concurrent
            .min(snapshot.request.limits.max_concurrent_jobs)
            .min(active_limit.unwrap_or(u16::MAX));
        if active >= i64::from(limit) {
            return Err(JobStoreError::ConcurrentLimit);
        }
        tx.execute(
            "UPDATE job_attempts SET state = 'dispatched' WHERE job_id = ?1 AND attempt_id = ?2",
            params![job_id, attempt_id],
        )?;
        let snapshot = require_snapshot(&tx, job_id)?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub(crate) fn accept(
        &mut self,
        bound: &JobResultSubmission,
    ) -> Result<JobSnapshot, JobStoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let snapshot = require_snapshot(&tx, &bound.job_id)?;
        check_attempt(&snapshot, &bound.attempt_id)?;
        if snapshot.request.input_revision != bound.input_revision {
            return Err(JobStoreError::RevisionMismatch);
        }
        let (digest, encoded) =
            job::result_digest(&bound.result, snapshot.request.limits.max_result_bytes)?;
        if snapshot.state == JobState::Completed
            && snapshot.attempt.state == JobAttemptState::Completed
        {
            let (stored_digest, stored_json): (String, String) = tx.query_row(
                "SELECT result_digest, result_json FROM job_attempts WHERE attempt_id = ?1",
                [&bound.attempt_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if digest != stored_digest || encoded != stored_json {
                return Err(JobStoreError::ResultConflict);
            }
            tx.commit()?;
            return Ok(snapshot);
        }
        if snapshot.state != JobState::Pending
            || snapshot.attempt.state != JobAttemptState::Dispatched
        {
            return Err(JobStoreError::InvalidTransition(
                "result requires the active dispatched attempt",
            ));
        }
        tx.execute(
            "UPDATE job_attempts SET result_json = ?3, result_digest = ?4,
             state = 'completed', failure = NULL WHERE job_id = ?1 AND attempt_id = ?2",
            params![bound.job_id, bound.attempt_id, encoded, digest],
        )?;
        tx.execute(
            "UPDATE job_intents SET state = 'completed' WHERE job_id = ?1",
            [&bound.job_id],
        )?;
        let snapshot = require_snapshot(&tx, &bound.job_id)?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub(crate) fn finish(
        &mut self,
        job_id: &str,
        attempt_id: &str,
        state: JobAttemptState,
        detail: Option<&str>,
    ) -> Result<JobSnapshot, JobStoreError> {
        if !state.terminal() {
            return Err(JobStoreError::InvalidTransition(
                "finish requires a terminal state",
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let snapshot = require_snapshot(&tx, job_id)?;
        check_attempt(&snapshot, attempt_id)?;
        // Cleanup/failure signals racing a committed result cannot erase success.
        if snapshot.state == JobState::Completed || snapshot.attempt.state == state {
            tx.commit()?;
            return Ok(snapshot);
        }
        if state == JobAttemptState::Completed {
            return Err(JobStoreError::InvalidTransition(
                "success requires an accepted result",
            ));
        }
        if snapshot.attempt.state.terminal() {
            return Err(JobStoreError::InvalidTransition(
                "terminal attempt cannot be reclassified",
            ));
        }
        set_terminal(&tx, &snapshot, state, detail)?;
        let snapshot = require_snapshot(&tx, job_id)?;
        tx.commit()?;
        Ok(snapshot)
    }

    /// Cancellation is durable. Cancelling unknown intent must not erase the
    /// attempt's uncertain execution evidence or release its serialization fence.
    /// A previously accepted result wins and remains success.
    pub(crate) fn cancel(&mut self, job_id: &str) -> Result<JobSnapshot, JobStoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let snapshot = require_snapshot(&tx, job_id)?;
        if snapshot.state == JobState::Unknown {
            tx.execute(
                "UPDATE job_intents SET state = 'cancelled' WHERE job_id = ?1",
                [job_id],
            )?;
        } else if !matches!(snapshot.state, JobState::Completed | JobState::Cancelled) {
            set_terminal(
                &tx,
                &snapshot,
                JobAttemptState::Cancelled,
                Some("cancelled"),
            )?;
        }
        let snapshot = require_snapshot(&tx, job_id)?;
        tx.commit()?;
        Ok(snapshot)
    }

    pub(crate) fn ack(&mut self, job_id: &str) -> Result<JobSnapshot, JobStoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let snapshot = require_snapshot(&tx, job_id)?;
        if snapshot.state != JobState::Completed {
            return Err(JobStoreError::InvalidTransition(
                "acknowledgement requires a completed Job",
            ));
        }
        tx.execute(
            "UPDATE job_intents SET acknowledged = 1 WHERE job_id = ?1",
            [job_id],
        )?;
        let snapshot = require_snapshot(&tx, job_id)?;
        tx.commit()?;
        Ok(snapshot)
    }
}

fn check_attempt(snapshot: &JobSnapshot, attempt_id: &str) -> Result<(), JobStoreError> {
    if snapshot.attempt.attempt_id != attempt_id {
        return Err(JobStoreError::AttemptMismatch);
    }
    Ok(())
}

fn check_serialization(
    conn: &Connection,
    key: Option<&str>,
    job_id: &str,
) -> Result<(), JobStoreError> {
    if let Some(key) = key {
        let conflicting: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM job_intents i WHERE serialization_key = ?1
             AND job_id <> ?2 AND (state IN ('pending', 'unknown') OR EXISTS
                 (SELECT 1 FROM job_attempts a WHERE a.job_id = i.job_id AND a.state = 'unknown')))",
            params![key, job_id],
            |row| row.get(0),
        )?;
        if conflicting {
            return Err(JobStoreError::SerializationConflict(key.to_owned()));
        }
    }
    Ok(())
}

fn insert_attempt(
    conn: &Connection,
    request: &JobRequest,
    number: u8,
) -> Result<(), JobStoreError> {
    conn.execute(
        "INSERT INTO job_attempts (job_id, number, attempt_id, input_revision, state)
         VALUES (?1, ?2, ?3, ?4, 'reserved')",
        params![
            request.job_id,
            number,
            job::attempt_id(&request.job_id, number),
            request.input_revision
        ],
    )?;
    Ok(())
}

fn set_terminal(
    conn: &Connection,
    snapshot: &JobSnapshot,
    state: JobAttemptState,
    detail: Option<&str>,
) -> Result<(), JobStoreError> {
    let detail = detail.map(job::bounded_failure_detail);
    conn.execute(
        "UPDATE job_attempts SET state = ?3, failure = ?4 WHERE job_id = ?1 AND attempt_id = ?2",
        params![
            snapshot.request.job_id,
            snapshot.attempt.attempt_id,
            state.as_str(),
            detail
        ],
    )?;
    conn.execute(
        "UPDATE job_intents SET state = ?2 WHERE job_id = ?1",
        params![snapshot.request.job_id, state.as_str()],
    )?;
    Ok(())
}

fn require_snapshot(conn: &Connection, job_id: &str) -> Result<JobSnapshot, JobStoreError> {
    read_snapshot(conn, job_id)?.ok_or_else(|| JobStoreError::NotFound(job_id.to_owned()))
}

fn read_snapshot(conn: &Connection, job_id: &str) -> Result<Option<JobSnapshot>, JobStoreError> {
    let row = conn
        .query_row(
            "SELECT i.request_json, i.state, i.acknowledged, a.attempt_id, a.number,
         a.state, a.failure, a.result_json FROM job_intents i JOIN job_attempts a
         ON a.job_id = i.job_id AND a.number = i.current_attempt WHERE i.job_id = ?1",
            [job_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u8>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(request, state, acknowledged, attempt_id, number, attempt_state, failure, result)| {
            let state = match state.as_str() {
                "pending" => JobState::Pending,
                "completed" => JobState::Completed,
                "failed" => JobState::Failed,
                "unknown" => JobState::Unknown,
                "cancelled" => JobState::Cancelled,
                _ => {
                    return Err(JobStoreError::Corrupt(format!(
                        "unknown intent state `{state}`"
                    )));
                }
            };
            let attempt_state = match attempt_state.as_str() {
                "reserved" => JobAttemptState::Reserved,
                "dispatched" => JobAttemptState::Dispatched,
                "completed" => JobAttemptState::Completed,
                "failed" => JobAttemptState::Failed,
                "unknown" => JobAttemptState::Unknown,
                "cancelled" => JobAttemptState::Cancelled,
                _ => {
                    return Err(JobStoreError::Corrupt(format!(
                        "unknown attempt state `{attempt_state}`"
                    )));
                }
            };
            Ok(JobSnapshot {
                request: serde_json::from_str(&request)?,
                domain_grant: conn
                    .query_row(
                        "SELECT grant_json FROM job_domain_grants WHERE job_id = ?1",
                        [job_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                    .map(|s| serde_json::from_str(&s))
                    .transpose()?,
                state,
                acknowledged,
                attempt: JobAttempt {
                    attempt_id,
                    number,
                    state: attempt_state,
                    failure,
                },
                result: result.map(|json| serde_json::from_str(&json)).transpose()?,
            })
        },
    )
    .transpose()
}

const GRANT_SCHEMA: &str = "
CREATE TABLE job_domain_grants (
    job_id TEXT PRIMARY KEY NOT NULL REFERENCES job_intents(job_id),
    grant_json TEXT NOT NULL
);
CREATE TRIGGER job_grant_immutable BEFORE UPDATE ON job_domain_grants
BEGIN SELECT RAISE(ABORT, 'immutable Job domain grant'); END;
CREATE TRIGGER job_grant_retained BEFORE DELETE ON job_domain_grants
BEGIN SELECT RAISE(ABORT, 'retained Job domain grant'); END;
";

const SCHEMA: &str = "
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
    input_revision TEXT NOT NULL,
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
    job_id, number, attempt_id, input_revision ON job_attempts
BEGIN SELECT RAISE(ABORT, 'immutable Job attempt identity'); END;
CREATE TRIGGER job_result_immutable BEFORE UPDATE OF result_json, result_digest ON job_attempts
WHEN OLD.result_json IS NOT NULL AND
    (NEW.result_json IS NOT OLD.result_json OR NEW.result_digest IS NOT OLD.result_digest)
BEGIN SELECT RAISE(ABORT, 'immutable accepted Job result'); END;
";

#[cfg(test)]
mod tests {
    use super::*;
    use job::JobLimits;
    use serde_json::json;
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;

    fn request(id: &str) -> JobRequest {
        JobRequest {
            job_id: id.into(),
            purpose: "fixture".into(),
            input_revision: "revision-1".into(),
            input_ref: "fixture:input".into(),
            input: json!({"immutable": [1, 2, 3]}),
            instruction: "Produce a structured result".into(),
            profile: "builtin:default".into(),
            serialization_key: None,
            limits: JobLimits::default(),
        }
    }

    fn fixture() -> (TempDir, JobStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        (dir, store)
    }

    fn submission(snapshot: &JobSnapshot, result: Value) -> JobResultSubmission {
        JobResultSubmission {
            job_id: snapshot.request.job_id.clone(),
            attempt_id: snapshot.attempt.attempt_id.clone(),
            input_revision: snapshot.request.input_revision.clone(),
            result,
        }
    }

    fn dispatched(store: &mut JobStore, request: JobRequest) -> JobSnapshot {
        let reserved = store.reserve(request).unwrap();
        store
            .dispatch(&reserved.request.job_id, &reserved.attempt.attempt_id, 8)
            .unwrap()
    }

    #[test]
    fn database_configuration_schema_and_identity_constraints() {
        let (_dir, mut store) = fixture();
        let version: i64 = store
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        let foreign_keys: i64 = store
            .conn
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .unwrap();
        let synchronous: i64 = store
            .conn
            .pragma_query_value(None, "synchronous", |r| r.get(0))
            .unwrap();
        let journal: String = store
            .conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!((version, foreign_keys, synchronous), (SCHEMA_VERSION, 1, 2));
        assert_eq!(journal, "wal");
        store.reserve(request("identity")).unwrap();
        for sql in [
            "UPDATE job_intents SET request_json = '{}' WHERE job_id = 'identity'",
            "UPDATE job_intents SET fingerprint = 'changed' WHERE job_id = 'identity'",
            "UPDATE job_attempts SET input_revision = 'changed' WHERE job_id = 'identity'",
            "UPDATE job_attempts SET attempt_id = 'changed' WHERE job_id = 'identity'",
            "UPDATE job_intents SET current_attempt = 2 WHERE job_id = 'identity'",
            "INSERT INTO job_attempts (job_id, number, attempt_id, input_revision, state)
             VALUES ('absent', 1, 'absent:attempt:1', 'r', 'reserved')",
        ] {
            assert!(
                store.conn.execute_batch(sql).is_err(),
                "constraint accepted {sql}"
            );
        }
        let mut check = store.conn.prepare("PRAGMA foreign_key_check").unwrap();
        assert!(!check.exists([]).unwrap());
    }

    #[test]
    fn schema_version_is_rejected_without_recovery_or_upgrade() {
        let (dir, mut store) = fixture();
        dispatched(&mut store, request("running"));
        store.conn.pragma_update(None, "user_version", 42).unwrap();
        drop(store);
        let path = dir.path().join("jobs.sqlite3");
        assert!(matches!(
            JobStore::open(&path),
            Err(JobStoreError::UnsupportedSchema(42))
        ));
        let conn = Connection::open(path).unwrap();
        let state: String = conn
            .query_row("SELECT state FROM job_attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "dispatched");
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            42
        );
    }

    #[test]
    fn exact_replay_is_stable_and_every_intent_change_conflicts() {
        let (dir, mut store) = fixture();
        assert!(matches!(
            store.get("absent"),
            Err(JobStoreError::NotFound(_))
        ));
        let original = request("replay");
        let snapshot = store.reserve(original.clone()).unwrap();
        assert_eq!(snapshot.state, JobState::Pending);
        assert_eq!(snapshot.attempt.number, 1);
        assert_eq!(snapshot.attempt.attempt_id, job::attempt_id("replay", 1));
        assert_eq!(snapshot.attempt.state, JobAttemptState::Reserved);
        assert_eq!(snapshot.result, None);
        assert!(!snapshot.acknowledged);
        assert_eq!(store.reserve(original.clone()).unwrap(), snapshot);
        let mut variants = Vec::new();
        let mut changed = original.clone();
        changed.input_revision = "revision-2".into();
        variants.push(changed);
        let mut changed = original.clone();
        changed.input = json!({"changed": true});
        variants.push(changed);
        let mut changed = original.clone();
        changed.input_ref = "other:input".into();
        variants.push(changed);
        let mut changed = original.clone();
        changed.profile = "builtin:coder".into();
        variants.push(changed);
        let mut changed = original.clone();
        changed.instruction = "Different instruction".into();
        variants.push(changed);
        let mut changed = original.clone();
        changed.purpose = "other_purpose".into();
        variants.push(changed);
        let mut changed = original.clone();
        changed.serialization_key = Some("key".into());
        variants.push(changed);
        let mut changed = original.clone();
        changed.limits.max_attempts = 3;
        variants.push(changed);
        for variant in variants {
            assert!(matches!(
                store.reserve(variant),
                Err(JobStoreError::IntentConflict(_))
            ));
        }
        drop(store);
        let mut reopened = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        assert_eq!(reopened.reserve(original).unwrap(), snapshot);
        assert_eq!(reopened.pending().unwrap(), vec![snapshot]);
        let count: i64 = reopened
            .conn
            .query_row("SELECT COUNT(*) FROM job_attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn invalid_requests_never_create_intent_or_attempt() {
        let (_dir, mut store) = fixture();
        let mut invalid = request("invalid");
        invalid.limits.max_attempts = 0;
        assert!(matches!(store.reserve(invalid), Err(JobStoreError::Job(_))));
        let mut invalid = request("invalid");
        invalid.profile = "/tmp/raw-profile.toml".into();
        assert!(matches!(store.reserve(invalid), Err(JobStoreError::Job(_))));
        let mut invalid = request("invalid");
        invalid.input = json!("x".repeat(job::MAX_JOB_INPUT_BYTES));
        assert!(matches!(store.reserve(invalid), Err(JobStoreError::Job(_))));
        assert!(store.pending().unwrap().is_empty());
        assert!(matches!(
            store.get("invalid"),
            Err(JobStoreError::NotFound(_))
        ));
    }

    #[test]
    fn retry_is_explicit_bounded_and_preserves_prior_attempt() {
        let (dir, mut store) = fixture();
        let mut original = request("retry");
        original.limits.max_attempts = 2;
        let first = dispatched(&mut store, original.clone());
        assert!(store.retry("retry").is_err());
        let failed = store
            .finish(
                "retry",
                &first.attempt.attempt_id,
                JobAttemptState::Failed,
                Some("timeout"),
            )
            .unwrap();
        assert_eq!(store.reserve(original.clone()).unwrap(), failed);
        let second = store.retry("retry").unwrap();
        assert_eq!(second.request, original);
        assert_eq!(second.attempt.number, 2);
        assert_eq!(second.attempt.state, JobAttemptState::Reserved);
        assert_eq!(second.attempt.failure, None);
        assert_eq!(second.result, None);
        assert!(matches!(
            store.accept(&submission(&first, json!({}))),
            Err(JobStoreError::AttemptMismatch)
        ));
        assert!(matches!(
            store.finish(
                "retry",
                &first.attempt.attempt_id,
                JobAttemptState::Failed,
                None
            ),
            Err(JobStoreError::AttemptMismatch)
        ));
        assert!(matches!(
            store.dispatch("retry", &first.attempt.attempt_id, 8),
            Err(JobStoreError::AttemptMismatch)
        ));
        let prior: (String, String, String) = store.conn.query_row(
            "SELECT state, failure, input_revision FROM job_attempts WHERE job_id = 'retry' AND number = 1",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(
            prior,
            ("failed".into(), "timeout".into(), "revision-1".into())
        );
        store
            .finish(
                "retry",
                &second.attempt.attempt_id,
                JobAttemptState::Failed,
                None,
            )
            .unwrap();
        assert!(matches!(
            store.retry("retry"),
            Err(JobStoreError::InvalidTransition("attempt limit reached"))
        ));
        drop(store);
        let reopened = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        assert_eq!(reopened.get("retry").unwrap().attempt.number, 2);
    }

    #[test]
    fn result_fences_size_and_replay_are_enforced_transactionally() {
        let (_dir, mut store) = fixture();
        let mut req = request("result");
        req.limits.max_result_bytes = 32;
        let reserved = store.reserve(req).unwrap();
        let valid = submission(&reserved, json!({"ok": true}));
        assert!(store.accept(&valid).is_err());
        let active = store
            .dispatch("result", &reserved.attempt.attempt_id, 8)
            .unwrap();
        assert!(store.pending().unwrap().is_empty());
        let mut invalid = valid.clone();
        invalid.attempt_id = "wrong-attempt".into();
        assert!(matches!(
            store.accept(&invalid),
            Err(JobStoreError::AttemptMismatch)
        ));
        let mut invalid = valid.clone();
        invalid.input_revision = "revision-2".into();
        assert!(matches!(
            store.accept(&invalid),
            Err(JobStoreError::RevisionMismatch)
        ));
        let mut invalid = valid.clone();
        invalid.result = json!("x".repeat(32));
        assert!(matches!(store.accept(&invalid), Err(JobStoreError::Job(_))));
        assert_eq!(store.get("result").unwrap(), active);
        let completed = store.accept(&valid).unwrap();
        assert_eq!(completed.state, JobState::Completed);
        assert_eq!(completed.attempt.state, JobAttemptState::Completed);
        assert_eq!(completed.result, Some(valid.result.clone()));
        assert_eq!(store.accept(&valid).unwrap(), completed);
        let (digest, json): (String, String) = store
            .conn
            .query_row(
                "SELECT result_digest, result_json FROM job_attempts WHERE job_id = 'result'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (digest, json),
            job::result_digest(&valid.result, 32).unwrap()
        );
        let mut changed = valid.clone();
        changed.result = json!({"ok": false});
        assert!(matches!(
            store.accept(&changed),
            Err(JobStoreError::ResultConflict)
        ));
        assert!(store.conn.execute("UPDATE job_attempts SET result_json = 'null', result_digest = 'changed' WHERE job_id = 'result'", []).is_err());
        assert_eq!(store.get("result").unwrap(), completed);
    }

    #[test]
    fn exact_byte_boundary_and_json_null_are_accepted() {
        let (_dir, mut store) = fixture();
        let mut req = request("boundary");
        req.limits.max_result_bytes = 4;
        let active = dispatched(&mut store, req);
        let completed = store.accept(&submission(&active, Value::Null)).unwrap();
        assert_eq!(completed.result, Some(Value::Null));
        let mut req = request("utf8");
        req.limits.max_result_bytes = 4;
        let active = dispatched(&mut store, req);
        assert!(store.accept(&submission(&active, json!("猫"))).is_err());
        assert_eq!(store.get("utf8").unwrap().state, JobState::Pending);
    }

    #[test]
    fn only_structured_result_is_success_and_cleanup_cannot_erase_it() {
        let (_dir, mut store) = fixture();
        let active = dispatched(&mut store, request("success"));
        for state in [
            JobAttemptState::Completed,
            JobAttemptState::Reserved,
            JobAttemptState::Dispatched,
        ] {
            assert!(
                store
                    .finish(
                        "success",
                        &active.attempt.attempt_id,
                        state,
                        Some("final prose")
                    )
                    .is_err()
            );
        }
        assert!(store.ack("success").is_err());
        let completed = store
            .accept(&submission(&active, json!({"value": 1})))
            .unwrap();
        for state in [
            JobAttemptState::Failed,
            JobAttemptState::Unknown,
            JobAttemptState::Cancelled,
            JobAttemptState::Completed,
        ] {
            assert_eq!(
                store
                    .finish(
                        "success",
                        &active.attempt.attempt_id,
                        state,
                        Some("cleanup error")
                    )
                    .unwrap(),
                completed
            );
        }
        assert_eq!(store.cancel("success").unwrap(), completed);
        assert!(store.retry("success").is_err());
    }

    #[test]
    fn consumer_acknowledgement_is_independent_durable_and_idempotent() {
        let (dir, mut store) = fixture();
        let active = dispatched(&mut store, request("ack"));
        let accepted = store
            .accept(&submission(&active, json!({"result": "retained"})))
            .unwrap();
        assert!(!accepted.acknowledged);
        drop(store);
        let mut reopened = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        assert_eq!(reopened.get("ack").unwrap(), accepted.clone());
        assert!(reopened.pending().unwrap().is_empty());
        assert!(reopened.retry("ack").is_err());
        let acknowledged = reopened.ack("ack").unwrap();
        assert!(acknowledged.acknowledged);
        assert_eq!(acknowledged.result, accepted.result);
        assert_eq!(reopened.ack("ack").unwrap(), acknowledged);
        assert_eq!(
            reopened
                .accept(&submission(&active, json!({"result": "retained"})))
                .unwrap(),
            acknowledged
        );
        drop(reopened);
        let mut reopened = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        assert_eq!(reopened.reserve(request("ack")).unwrap(), acknowledged);
    }

    #[test]
    fn reopen_keeps_reserved_and_marks_unaccepted_dispatch_unknown() {
        let (dir, mut store) = fixture();
        let pending = store.reserve(request("pending")).unwrap();
        let active = dispatched(&mut store, request("interrupted"));
        let failed = store.reserve(request("failed")).unwrap();
        store
            .finish(
                "failed",
                &failed.attempt.attempt_id,
                JobAttemptState::Failed,
                Some("known failure"),
            )
            .unwrap();
        drop(store);
        let mut reopened = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        assert_eq!(reopened.pending().unwrap(), vec![pending.clone()]);
        assert_eq!(reopened.get("pending").unwrap(), pending);
        let unknown = reopened.get("interrupted").unwrap();
        assert_eq!(unknown.state, JobState::Unknown);
        assert_eq!(unknown.attempt.state, JobAttemptState::Unknown);
        assert_eq!(unknown.result, None);
        assert!(unknown.attempt.failure.is_some());
        assert_eq!(reopened.reserve(request("interrupted")).unwrap(), unknown);
        assert!(reopened.retry("interrupted").is_err());
        assert!(
            reopened
                .accept(&submission(&active, json!({"late": true})))
                .is_err()
        );
        assert!(
            reopened
                .dispatch("interrupted", &active.attempt.attempt_id, 8)
                .is_err()
        );
        assert_eq!(reopened.get("failed").unwrap().state, JobState::Failed);
        drop(reopened);
        let reopened = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        assert_eq!(reopened.get("interrupted").unwrap(), unknown);
    }

    #[test]
    fn recovery_retains_accepted_result_in_dispatched_fixture() {
        let (dir, mut store) = fixture();
        let active = dispatched(&mut store, request("accepted-fixture"));
        let bound = submission(&active, json!({"accepted": true}));
        // Explicit recovery fixture, not a possible half-commit of accept():
        // accept() updates both states and the result in one transaction.
        let (digest, encoded) = job::result_digest(&bound.result, 1024).unwrap();
        store.conn.execute(
            "UPDATE job_attempts SET result_json = ?1, result_digest = ?2 WHERE attempt_id = ?3",
            params![encoded, digest, bound.attempt_id],
        ).unwrap();
        drop(store);
        let mut reopened = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        let retained = reopened.get("accepted-fixture").unwrap();
        assert_eq!(retained.state, JobState::Completed);
        assert_eq!(retained.attempt.state, JobAttemptState::Completed);
        assert_eq!(retained.result, Some(bound.result.clone()));
        assert_eq!(reopened.accept(&bound).unwrap(), retained);
        assert!(reopened.pending().unwrap().is_empty());
    }

    #[test]
    fn cancellation_never_resumes_and_rejects_late_results() {
        let (dir, mut store) = fixture();
        for (id, start) in [("reserved-cancel", false), ("dispatched-cancel", true)] {
            let snapshot = if start {
                dispatched(&mut store, request(id))
            } else {
                store.reserve(request(id)).unwrap()
            };
            let cancelled = store.cancel(id).unwrap();
            assert_eq!(cancelled.state, JobState::Cancelled);
            assert_eq!(cancelled.attempt.state, JobAttemptState::Cancelled);
            assert_eq!(store.cancel(id).unwrap(), cancelled);
            assert_eq!(store.reserve(request(id)).unwrap(), cancelled);
            assert!(store.retry(id).is_err());
            assert!(store.dispatch(id, &snapshot.attempt.attempt_id, 8).is_err());
            assert!(store.accept(&submission(&snapshot, json!({}))).is_err());
            assert!(
                store
                    .finish(
                        id,
                        &snapshot.attempt.attempt_id,
                        JobAttemptState::Failed,
                        None
                    )
                    .is_err()
            );
            assert!(store.ack(id).is_err());
        }
        drop(store);
        let mut reopened = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        assert!(reopened.pending().unwrap().is_empty());
        for id in ["reserved-cancel", "dispatched-cancel"] {
            assert_eq!(
                reopened.reserve(request(id)).unwrap().state,
                JobState::Cancelled
            );
        }
    }

    #[test]
    fn serialization_fences_pending_unknown_and_conflicting_retry() {
        let (dir, mut store) = fixture();
        let mut first = request("first");
        first.serialization_key = Some("resource".into());
        let mut second = request("second");
        second.serialization_key = Some("resource".into());
        let reserved = store.reserve(first.clone()).unwrap();
        assert_eq!(store.reserve(first.clone()).unwrap(), reserved);
        assert!(matches!(
            store.reserve(second.clone()),
            Err(JobStoreError::SerializationConflict(_))
        ));
        store
            .dispatch("first", &reserved.attempt.attempt_id, 8)
            .unwrap();
        drop(store);
        let mut reopened = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        assert!(matches!(
            reopened.reserve(second.clone()),
            Err(JobStoreError::SerializationConflict(_))
        ));
        let cancelled_unknown = reopened.cancel("first").unwrap();
        assert_eq!(cancelled_unknown.state, JobState::Cancelled);
        assert_eq!(cancelled_unknown.attempt.state, JobAttemptState::Unknown);
        assert!(matches!(
            reopened.reserve(second.clone()),
            Err(JobStoreError::SerializationConflict(_))
        ));
        assert!(reopened.retry("first").is_err());
        // Another independent resource is not fenced by this uncertain attempt.
        second.serialization_key = Some("independent-resource".into());
        let second_snapshot = reopened.reserve(second.clone()).unwrap();
        reopened
            .finish(
                "second",
                &second_snapshot.attempt.attempt_id,
                JobAttemptState::Failed,
                None,
            )
            .unwrap();
        let mut third = request("third");
        third.serialization_key = Some("independent-resource".into());
        reopened.reserve(third).unwrap();
        assert!(matches!(
            reopened.retry("second"),
            Err(JobStoreError::SerializationConflict(_))
        ));
        assert_eq!(reopened.get("second").unwrap().attempt.number, 1);
        reopened.cancel("third").unwrap();
        assert_eq!(reopened.retry("second").unwrap().attempt.number, 2);
    }

    #[test]
    fn dispatch_enforces_host_and_all_active_request_limits_without_double_start() {
        let (_dir, mut store) = fixture();
        let mut strict = request("strict");
        strict.limits.max_concurrent_jobs = 1;
        let strict = store.reserve(strict).unwrap();
        let other = store.reserve(request("other")).unwrap();
        assert!(matches!(
            store.dispatch("strict", &strict.attempt.attempt_id, 0),
            Err(JobStoreError::ConcurrentLimit)
        ));
        store
            .dispatch("strict", &strict.attempt.attempt_id, 8)
            .unwrap();
        assert!(matches!(
            store.dispatch("other", &other.attempt.attempt_id, 8),
            Err(JobStoreError::ConcurrentLimit)
        ));
        assert!(
            store
                .dispatch("strict", &strict.attempt.attempt_id, 8)
                .is_err()
        );
        store
            .finish(
                "strict",
                &strict.attempt.attempt_id,
                JobAttemptState::Failed,
                None,
            )
            .unwrap();
        store
            .dispatch("other", &other.attempt.attempt_id, 8)
            .unwrap();
        let strict = store.retry("strict").unwrap();
        assert!(matches!(
            store.dispatch("strict", &strict.attempt.attempt_id, 8),
            Err(JobStoreError::ConcurrentLimit)
        ));
        let host_bound = store.reserve(request("host-bound")).unwrap();
        assert!(matches!(
            store.dispatch("host-bound", &host_bound.attempt.attempt_id, 1),
            Err(JobStoreError::ConcurrentLimit)
        ));
        assert_eq!(
            store.get("host-bound").unwrap().attempt.state,
            JobAttemptState::Reserved
        );
        store.accept(&submission(&other, json!({}))).unwrap();
        store
            .dispatch("host-bound", &host_bound.attempt.attempt_id, 1)
            .unwrap();
    }

    #[test]
    fn competing_transactions_cannot_exceed_host_dispatch_limit() {
        let (dir, mut one) = fixture();
        // Open all fixture connections before dispatch: production has one
        // Host-leased connection and never invokes recovery beside live work.
        let mut two = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        let first = one.reserve(request("one")).unwrap();
        let second = two.reserve(request("two")).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let threads = [(one, first), (two, second)]
            .into_iter()
            .map(|(mut store, snapshot)| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.dispatch(&snapshot.request.job_id, &snapshot.attempt.attempt_id, 1)
                })
            })
            .collect::<Vec<_>>();
        let results = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(JobStoreError::ConcurrentLimit)))
                .count(),
            1
        );
        let conn = Connection::open(dir.path().join("jobs.sqlite3")).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM job_attempts WHERE state = 'dispatched'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn competing_reservations_enforce_singleflight_without_partial_intent() {
        let (dir, one) = fixture();
        let two = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let threads = [(one, "one"), (two, "two")]
            .into_iter()
            .map(|(mut store, id)| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut req = request(id);
                    req.serialization_key = Some("singleflight".into());
                    barrier.wait();
                    store.reserve(req)
                })
            })
            .collect::<Vec<_>>();
        let results = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(JobStoreError::SerializationConflict(_))))
                .count(),
            1
        );
        let reopened = JobStore::open(&dir.path().join("jobs.sqlite3")).unwrap();
        assert_eq!(reopened.pending().unwrap().len(), 1);
        let count: i64 = reopened
            .conn
            .query_row("SELECT COUNT(*) FROM job_attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn failure_detail_is_utf8_bounded_and_terminal_classification_is_stable() {
        let (_dir, mut store) = fixture();
        let reserved = store.reserve(request("failure")).unwrap();
        let failed = store
            .finish(
                "failure",
                &reserved.attempt.attempt_id,
                JobAttemptState::Failed,
                Some(&"猫".repeat(1024)),
            )
            .unwrap();
        let detail = failed.attempt.failure.as_ref().unwrap();
        assert!(detail.len() <= job::MAX_JOB_FAILURE_BYTES);
        assert!(detail.ends_with('猫'));
        assert_eq!(
            store
                .finish(
                    "failure",
                    &reserved.attempt.attempt_id,
                    JobAttemptState::Failed,
                    Some("different detail")
                )
                .unwrap(),
            failed
        );
        assert!(
            store
                .finish(
                    "failure",
                    &reserved.attempt.attempt_id,
                    JobAttemptState::Unknown,
                    None
                )
                .is_err()
        );
        assert!(
            store
                .finish(
                    "failure",
                    &reserved.attempt.attempt_id,
                    JobAttemptState::Completed,
                    None
                )
                .is_err()
        );
        assert!(store.accept(&submission(&reserved, json!({}))).is_err());
        assert!(store.ack("failure").is_err());
        assert_eq!(store.cancel("failure").unwrap().state, JobState::Cancelled);
        assert!(store.retry("failure").is_err());
    }

    #[test]
    fn injected_sql_failures_roll_back_reserve_result_and_retry() {
        let (_dir, mut store) = fixture();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER fixture_abort_attempt BEFORE INSERT ON job_attempts
             BEGIN SELECT RAISE(ABORT, 'fixture insertion failure'); END;",
            )
            .unwrap();
        assert!(matches!(
            store.reserve(request("atomic")),
            Err(JobStoreError::Sqlite(_))
        ));
        assert!(store.get("atomic").is_err());
        store
            .conn
            .execute_batch("DROP TRIGGER fixture_abort_attempt")
            .unwrap();
        let active = dispatched(&mut store, request("atomic"));
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER fixture_abort_intent BEFORE UPDATE OF state ON job_intents
             BEGIN SELECT RAISE(ABORT, 'fixture intent failure'); END;",
            )
            .unwrap();
        let bound = submission(&active, json!({"atomic": true}));
        assert!(matches!(
            store.accept(&bound),
            Err(JobStoreError::Sqlite(_))
        ));
        assert_eq!(store.get("atomic").unwrap(), active);
        assert!(matches!(
            store.cancel("atomic"),
            Err(JobStoreError::Sqlite(_))
        ));
        assert_eq!(store.get("atomic").unwrap(), active);
        store
            .conn
            .execute_batch("DROP TRIGGER fixture_abort_intent")
            .unwrap();
        let failed = store
            .finish(
                "atomic",
                &active.attempt.attempt_id,
                JobAttemptState::Failed,
                Some("known failure"),
            )
            .unwrap();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER fixture_abort_intent BEFORE UPDATE OF state ON job_intents
             BEGIN SELECT RAISE(ABORT, 'fixture intent failure'); END;",
            )
            .unwrap();
        assert!(matches!(
            store.retry("atomic"),
            Err(JobStoreError::Sqlite(_))
        ));
        assert_eq!(store.get("atomic").unwrap(), failed);
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM job_attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        store
            .conn
            .execute_batch("DROP TRIGGER fixture_abort_intent")
            .unwrap();
        let next = store.retry("atomic").unwrap();
        store
            .dispatch("atomic", &next.attempt.attempt_id, 8)
            .unwrap();
        assert_eq!(
            store
                .accept(&submission(&next, bound.result))
                .unwrap()
                .state,
            JobState::Completed
        );
    }

    #[test]
    fn unknown_job_operations_do_not_create_records() {
        let (_dir, mut store) = fixture();
        assert!(matches!(
            store.retry("missing"),
            Err(JobStoreError::NotFound(_))
        ));
        assert!(matches!(
            store.dispatch("missing", "attempt", 8),
            Err(JobStoreError::NotFound(_))
        ));
        assert!(matches!(
            store.finish("missing", "attempt", JobAttemptState::Failed, None),
            Err(JobStoreError::NotFound(_))
        ));
        assert!(matches!(
            store.cancel("missing"),
            Err(JobStoreError::NotFound(_))
        ));
        assert!(matches!(
            store.ack("missing"),
            Err(JobStoreError::NotFound(_))
        ));
        let absent = JobResultSubmission {
            job_id: "missing".into(),
            attempt_id: "attempt".into(),
            input_revision: "r".into(),
            result: json!({}),
        };
        assert!(matches!(
            store.accept(&absent),
            Err(JobStoreError::NotFound(_))
        ));
        assert!(store.pending().unwrap().is_empty());
    }
}
