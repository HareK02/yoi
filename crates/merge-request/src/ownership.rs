use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkerAssignedEvent {
    pub event_id: String,
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    pub sequence: u64,
    pub operation_id: String,
    pub previous_operation_id: Option<String>,
    pub worker: Option<WorkerIdentity>,
    pub assigned_by: WorkerIdentity,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SetMergeRequestWorker {
    pub workspace_id: String,
    pub merge_request_id: String,
    pub operation_id: String,
    pub expected_operation_id: Option<String>,
    pub worker: Option<WorkerIdentity>,
    pub assigned_by: WorkerIdentity,
    pub now: DateTime<Utc>,
}

impl MergeRequest {
    /// Requires the full thread. An absent event means ownership was never recorded;
    /// an event with no Worker is an explicit release and retains its CAS identity.
    pub fn current_worker(&self) -> Option<&WorkerAssignedEvent> {
        self.thread.iter().rev().find_map(|event| match event {
            MergeRequestThreadEvent::WorkerAssigned(event) => Some(event),
            _ => None,
        })
    }
}

impl MergeRequestStore {
    pub fn current_worker(
        &self,
        workspace_id: &str,
        merge_request_id: &str,
    ) -> Result<Option<WorkerAssignedEvent>, MergeRequestError> {
        Ok(self
            .get_by_id(workspace_id, merge_request_id)?
            .current_worker()
            .cloned())
    }

    /// Assignment is not a permission grant. The Host must authenticate the actor
    /// and admit the desired Worker's resources, including on historical replay.
    pub fn set_worker(
        &self,
        i: SetMergeRequestWorker,
        admit: impl Fn(&Connection) -> Result<(), MergeRequestError>,
    ) -> Result<WorkerAssignedEvent, MergeRequestError> {
        self.set_worker_with_admission(i, admit)
    }

    /// Admission runs under the IMMEDIATE writer transaction before replay or CAS.
    /// Use only the supplied connection; do not reenter this store or call providers.
    /// Replay returns the original receipt, not current ownership. Server-generated
    /// time is not part of the fingerprint. Transfer changes neither lifecycle nor
    /// review grants/evidence, so an in-flight independent review remains valid.
    pub fn set_worker_with_admission(
        &self,
        i: SetMergeRequestWorker,
        admit: impl Fn(&Connection) -> Result<(), MergeRequestError>,
    ) -> Result<WorkerAssignedEvent, MergeRequestError> {
        for (name, value) in [
            ("workspace_id", i.workspace_id.as_str()),
            ("merge_request_id", &i.merge_request_id),
            ("operation_id", &i.operation_id),
            ("actor runtime_id", &i.assigned_by.runtime_id),
            ("actor worker_id", &i.assigned_by.worker_id),
        ] {
            nonempty(name, value)?;
        }
        if let Some(operation_id) = &i.expected_operation_id {
            nonempty("expected_operation_id", operation_id)?;
        }
        if let Some(worker) = &i.worker {
            nonempty("worker runtime_id", &worker.runtime_id)?;
            nonempty("worker worker_id", &worker.worker_id)?;
        }
        let mut connection = self.lock()?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        admit(&transaction)?;
        let mr = load_mr(&transaction, &i.workspace_id, &i.merge_request_id)?
            .ok_or(MergeRequestError::NotFound)?;
        let existing: Option<(String, String, String)> = transaction.query_row(
            "SELECT merge_request_id,kind,payload_json FROM merge_request_thread_events WHERE workspace_id=?1 AND operation_id=?2",
            params![i.workspace_id, i.operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?;
        if let Some((mr_id, kind, payload)) = existing {
            if mr_id == i.merge_request_id && kind == "worker_assigned" {
                let event: WorkerAssignedEvent = json(&payload)?;
                if event.previous_operation_id == i.expected_operation_id
                    && event.worker == i.worker
                    && event.assigned_by == i.assigned_by
                {
                    return Ok(event);
                }
            }
            return Err(MergeRequestError::Conflict(
                "worker assignment operation fingerprint mismatch".into(),
            ));
        }
        if mr.current_worker().map(|event| &event.operation_id) != i.expected_operation_id.as_ref()
        {
            return Err(MergeRequestError::Conflict(
                "current MR Worker assignment changed".into(),
            ));
        }
        let event = WorkerAssignedEvent {
            event_id: Uuid::now_v7().to_string(),
            sequence: next_seq(&transaction, &i.workspace_id, &i.merge_request_id)?,
            operation_id: i.operation_id,
            previous_operation_id: i.expected_operation_id,
            worker: i.worker,
            assigned_by: i.assigned_by,
            created_at: i.now,
        };
        insert_event(
            &transaction,
            &i.workspace_id,
            &i.merge_request_id,
            "worker_assigned",
            &event,
            i.now,
            Some(&event.operation_id),
        )?;
        transaction.execute(
            "UPDATE merge_requests SET updated_at=?3 WHERE workspace_id=?1 AND merge_request_id=?2",
            params![i.workspace_id, i.merge_request_id, i.now.to_rfc3339()],
        )?;
        transaction.commit()?;
        Ok(event)
    }

    pub(super) fn current_worker_auth(
        &self,
        auth: &MergeRequestAuth,
        mr: &MergeRequest,
    ) -> Result<(), MergeRequestError> {
        self.repo(auth, &mr.repository_id)?;
        let current = mr.current_worker().ok_or_else(|| {
            MergeRequestError::Unauthorized(
                "MR Worker ownership is unassigned; legacy ownership must be explicitly assigned"
                    .into(),
            )
        })?;
        if current.operation_id != auth.assignment_id
            || current.worker.as_ref() != Some(&auth.actor())
        {
            return Err(MergeRequestError::Unauthorized(
                "not current MR Worker or stale ownership operation".into(),
            ));
        }
        Ok(())
    }
}

/// Historical Ticket assignments cannot establish MR ownership. This migration
/// preserves every event and grant without manufacturing a Worker assignment.
pub(super) fn migrate_v14_worker_ownership(c: &Connection) -> Result<(), MergeRequestError> {
    let transaction = c.unchecked_transaction()?;
    verify(&transaction)?;
    verify_lifecycle_schema(&transaction)?;
    // The Workspace notification owner installs triggers on this table. Retain
    // those definitions across the rebuild without replaying historical events.
    let event_triggers = {
        let mut statement = transaction.prepare(
            "SELECT sql FROM sqlite_master WHERE type='trigger' AND tbl_name IN ('merge_request_thread_events','merge_request_review_grants') ORDER BY name",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    // Preserve the referencing grants before DROP can cascade away evidence.
    // Lifecycle triggers reference the event table and are restored after rebuilding.
    transaction.execute_batch(
        "
        DROP TRIGGER merge_request_lifecycle_transition;
        DROP TRIGGER merge_request_lifecycle_evidence_insert;
        DROP TRIGGER merge_request_lifecycle_evidence_update;
        CREATE TEMP TABLE merge_request_saved_events AS SELECT * FROM merge_request_thread_events;
        CREATE TEMP TABLE merge_request_saved_grants AS SELECT * FROM merge_request_review_grants;
        DROP TABLE merge_request_review_grants;
        DROP TABLE merge_request_thread_events;
    ",
    )?;
    worker_event_tables(&transaction)?;
    transaction.execute_batch(
        "
        INSERT INTO merge_request_thread_events SELECT * FROM merge_request_saved_events;
        INSERT INTO merge_request_review_grants SELECT * FROM merge_request_saved_grants;
        DROP TABLE merge_request_saved_events;
        DROP TABLE merge_request_saved_grants;
        UPDATE merge_request_schema SET version=15 WHERE singleton=1;
    ",
    )?;
    lifecycle::lifecycle_triggers(&transaction)?;
    for trigger in event_triggers {
        transaction.execute_batch(&trigger)?;
    }
    verify(&transaction)?;
    verify_lifecycle_schema(&transaction)?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn worker_event_tables(c: &Connection) -> Result<(), MergeRequestError> {
    c.execute_batch("
        CREATE TABLE merge_request_thread_events(
            workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,event_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            kind TEXT NOT NULL CHECK(kind IN('review_requested','review','review_revoked','review_cancelled','comment','merge','worker_assigned')),
            payload_json TEXT NOT NULL,operation_id TEXT,created_at TEXT NOT NULL,
            PRIMARY KEY(workspace_id,merge_request_id,event_id),
            UNIQUE(workspace_id,merge_request_id,sequence),
            FOREIGN KEY(workspace_id,merge_request_id) REFERENCES merge_requests(workspace_id,merge_request_id) ON DELETE CASCADE);
        CREATE UNIQUE INDEX merge_request_merge_operations ON merge_request_thread_events(workspace_id,operation_id) WHERE operation_id IS NOT NULL;
        CREATE TABLE merge_request_review_grants(
            workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,request_event_id TEXT NOT NULL,
            subject_ref TEXT NOT NULL,reviewer_runtime_id TEXT NOT NULL,reviewer_worker_id TEXT NOT NULL,
            capability_token TEXT PRIMARY KEY,issued_at TEXT NOT NULL,consumed_at TEXT,revoked_at TEXT,
            status TEXT NOT NULL CHECK(status IN('issued','consumed','revoked')),
            FOREIGN KEY(workspace_id,merge_request_id,request_event_id) REFERENCES merge_request_thread_events(workspace_id,merge_request_id,event_id) ON DELETE CASCADE);
    ")?;
    Ok(())
}
