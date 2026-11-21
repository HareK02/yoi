//! Publication validates current approval and records confirmed completion.
//! Historical publication rows are evidence only, never live target reservations.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicationBinding {
    pub workspace_id: String,
    pub merge_request_id: String,
    pub operation_id: String,
    pub repository_id: String,
    pub repository_source_fingerprint: String,
    pub alias: String,
    /// Canonical refs/heads/... spelling, never an arbitrary refspec.
    pub target: String,
    pub source: String,
    pub approval_event_id: String,
    pub owner_operation_id: String,
    pub worker: WorkerIdentity,
    pub working_directory_id: String,
    pub connection_id: String,
    pub target_before: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublicationOutcome {
    /// Historical unconfirmed operation, not a live target reservation.
    Unconfirmed {},
    /// Runtime has durably confirmed no publication by this operation.
    NotPublished { reason: String },
    /// Immutable provider evidence. A later target movement does not replace it.
    Published {
        target_after: String,
        observed_at: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Publication {
    pub binding: PublicationBinding,
    pub ticket_content_digest: String,
    pub outcome: PublicationOutcome,
    pub created_at: String,
}

/// Provider-verified facts supplied only by Backend, never by a model-facing DTO.
/// Manual completion records the observed target in both endpoints; it does not
/// claim to have observed the earlier Git update. The approved source may be an ancestor.
#[derive(Debug, Clone)]
pub struct ConfirmedMergeRequestPublication {
    pub workspace_id: String,
    pub merge_request_id: String,
    pub operation_id: String,
    pub repository_id: String,
    pub target: String,
    pub source: String,
    pub approval_event_id: String,
    pub owner_operation_id: String,
    pub worker: WorkerIdentity,
    pub target_before: String,
    pub target_after: String,
}

impl From<&PublicationBinding> for ConfirmedMergeRequestPublication {
    fn from(b: &PublicationBinding) -> Self {
        Self {
            workspace_id: b.workspace_id.clone(),
            merge_request_id: b.merge_request_id.clone(),
            operation_id: b.operation_id.clone(),
            repository_id: b.repository_id.clone(),
            target: b.target.clone(),
            source: b.source.clone(),
            approval_event_id: b.approval_event_id.clone(),
            owner_operation_id: b.owner_operation_id.clone(),
            worker: b.worker.clone(),
            target_before: b.target_before.clone(),
            target_after: b.source.clone(),
        }
    }
}

fn load(
    c: &Connection,
    workspace: &str,
    operation: &str,
) -> Result<Option<Publication>, MergeRequestError> {
    c.query_row("SELECT payload_json FROM merge_request_publications WHERE workspace_id=?1 AND operation_id=?2",
        params![workspace, operation], |row| row.get::<_, String>(0)).optional()?.map(|v| json(&v)).transpose()
}

fn normalized_target(selector: &str) -> String {
    selector
        .strip_prefix("refs/heads/")
        .unwrap_or(selector)
        .to_string()
}

impl MergeRequestStore {
    pub fn publication(
        &self,
        workspace: &str,
        operation: &str,
    ) -> Result<Option<Publication>, MergeRequestError> {
        let c = self.lock()?;
        load(&c, workspace, operation)
    }

    /// Historical evidence only; these rows never authorize or resume publication.
    pub fn publications(
        &self,
        workspace: &str,
        mr: &str,
    ) -> Result<Vec<Publication>, MergeRequestError> {
        let c = self.lock()?;
        let mut statement = c.prepare("SELECT payload_json FROM merge_request_publications WHERE workspace_id=?1 AND merge_request_id=?2 ORDER BY rowid DESC LIMIT 100")?;
        let rows = statement
            .query_map(params![workspace, mr], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter().map(|r| json(&r)).collect()
    }

    /// Backend-owned attention after an attempted automatic publication. This
    /// does not impersonate a Worker or grant permission to retry the effect.
    pub fn record_publication_problem(
        &self,
        workspace: &str,
        mr_id: &str,
        body: &str,
        now: DateTime<Utc>,
    ) -> Result<CommentEvent, MergeRequestError> {
        nonempty("publication comment", body)?;
        bounded_body("publication comment", body)?;
        let mut c = self.lock()?;
        let t = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        load_mr(&t, workspace, mr_id)?.ok_or(MergeRequestError::NotFound)?;
        let event = CommentEvent {
            operation_id: None,
            owner_operation_id: None,
            event_id: Uuid::now_v7().to_string(),
            sequence: next_seq(&t, workspace, mr_id)?,
            body: body.into(),
            author: None,
            created_at: now,
        };
        insert_event(&t, workspace, mr_id, "comment", &event, now, None)?;
        t.execute(
            "UPDATE merge_requests SET updated_at=?3 WHERE workspace_id=?1 AND merge_request_id=?2",
            params![workspace, mr_id, now.to_rfc3339()],
        )?;
        t.commit()?;
        Ok(event)
    }

    /// Validate immediately before dispatch without persisting an operation.
    pub fn validate_publication(
        &self,
        binding: &PublicationBinding,
        admit: impl Fn(&Connection) -> Result<(), MergeRequestError>,
    ) -> Result<String, MergeRequestError> {
        nonempty("source", &binding.source)?;
        nonempty("target_before", &binding.target_before)?;
        if !binding.target.starts_with("refs/heads/") || binding.target == "refs/heads/" {
            return Err(MergeRequestError::Validation(
                "publication requires a canonical branch".into(),
            ));
        }
        let mut c = self.lock()?;
        let t = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        admit(&t)?;
        let mr = load_mr(&t, &binding.workspace_id, &binding.merge_request_id)?
            .ok_or(MergeRequestError::NotFound)?;
        self.current_worker_auth(
            &MergeRequestAuth {
                workspace_id: binding.workspace_id.clone(),
                repository_id: binding.repository_id.clone(),
                runtime_id: binding.worker.runtime_id.clone(),
                worker_id: binding.worker.worker_id.clone(),
                assignment_id: binding.owner_operation_id.clone(),
            },
            &mr,
        )?;
        if !matches!(mr.state, MergeRequestState::Integration { .. })
            || mr.selector_from.is_none()
            || normalized_target(&mr.selector_to) != normalized_target(&binding.target)
        {
            return Err(MergeRequestError::NotReady(
                "MR lifecycle or target does not permit publication".into(),
            ));
        }
        let approval = mr
            .effective_review(&binding.source)
            .filter(|r| {
                r.event_id == binding.approval_event_id && r.decision == ReviewDecision::Approve
            })
            .ok_or_else(|| {
                MergeRequestError::NotReady(
                    "publication requires the effective exact-source approval".into(),
                )
            })?;
        let ticket = mr.ticket_ids.first().ok_or(MergeRequestError::NotFound)?;
        let digest = current_ticket_content_digest(&t, &binding.workspace_id, ticket)?
            .ok_or(MergeRequestError::NotFound)?;
        if digest != approval.ticket_content_digest {
            return Err(MergeRequestError::NotReady(
                "publication approval predates requirements".into(),
            ));
        }
        Ok(digest)
    }

    /// Called by Backend only after a confirmed fixed-source Git result. This is
    /// a completion record, not a receipt or an authorization to retry Git.
    pub fn record_confirmed_publication(
        &self,
        b: &PublicationBinding,
        ticket_content_digest: &str,
        now: DateTime<Utc>,
    ) -> Result<MergeEvent, MergeRequestError> {
        self.record_verified_completion(&b.into(), ticket_content_digest, now, |_, _| Ok(()))
    }

    /// Both automatic success and explicitly requested manual completion use this
    /// atomic outcome record. Manual admission rechecks current authority and owner
    /// under the writer transaction, including replay; it must not call providers.
    pub fn record_verified_completion(
        &self,
        b: &ConfirmedMergeRequestPublication,
        ticket_content_digest: &str,
        now: DateTime<Utc>,
        admit: impl Fn(&Connection, &MergeRequest) -> Result<(), MergeRequestError>,
    ) -> Result<MergeEvent, MergeRequestError> {
        for (name, value) in [
            ("operation_id", b.operation_id.as_str()),
            ("source", b.source.as_str()),
            ("target_before", b.target_before.as_str()),
            ("target_after", b.target_after.as_str()),
        ] {
            nonempty(name, value)?;
        }
        let workspace = b.workspace_id.as_str();
        let operation = b.operation_id.as_str();
        let mut c = self.lock()?;
        let t = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mr = load_mr(&t, workspace, &b.merge_request_id)?.ok_or(MergeRequestError::NotFound)?;
        admit(&t, &mr)?;
        if mr.repository_id != b.repository_id
            || normalized_target(&mr.selector_to) != normalized_target(&b.target)
        {
            return Err(MergeRequestError::Conflict(
                "completion Repository or target mismatch".into(),
            ));
        }
        if let Some(event) = mr.thread.iter().find_map(|e| match e {
            MergeRequestThreadEvent::Merge(e) if e.operation_id == operation => Some(e),
            _ => None,
        }) {
            if event.owner_operation_id.as_deref() != Some(b.owner_operation_id.as_str())
                || event.approval_event_id != b.approval_event_id
                || event.approved_source_ref != b.source
                || event.target_ref_before != b.target_before
                || event.target_ref_after != b.target_after
                || event.merged_by != b.worker
            {
                return Err(MergeRequestError::Conflict(
                    "completion fingerprint mismatch".into(),
                ));
            }
            return Ok(event.clone());
        }
        let MergeRequestState::Integration { started_by } = &mr.state else {
            return Err(MergeRequestError::NotReady(
                "Git publication confirmed, but MR is no longer in integration".into(),
            ));
        };
        let ticket = mr.ticket_ids.first().ok_or(MergeRequestError::NotFound)?;
        if current_ticket_content_digest(&t, workspace, ticket)?.as_deref()
            != Some(ticket_content_digest)
            || !mr.effective_review(&b.source).is_some_and(|r| {
                r.event_id == b.approval_event_id
                    && r.decision == ReviewDecision::Approve
                    && r.ticket_content_digest == ticket_content_digest
            })
        {
            return Err(MergeRequestError::NotReady(
                "Git publication confirmed, but requirements or approval changed".into(),
            ));
        }
        // Existing issued review capabilities may not create a later verdict on
        // a source that has already been published by this operation.
        t.execute("UPDATE merge_request_review_grants SET status='revoked',revoked_at=?3 WHERE workspace_id=?1 AND merge_request_id=?2 AND status='issued'",
            params![workspace, b.merge_request_id, now.to_rfc3339()])?;
        let event = MergeEvent {
            owner_operation_id: Some(b.owner_operation_id.clone()),
            event_id: Uuid::now_v7().to_string(),
            sequence: next_seq(&t, workspace, &b.merge_request_id)?,
            operation_id: operation.into(),
            approval_event_id: b.approval_event_id.clone(),
            approved_source_ref: b.source.clone(),
            target_ref_before: b.target_before.clone(),
            target_ref_after: b.target_after.clone(),
            strategy: MergeStrategy::FastForward,
            resolution: ConflictResolution::None,
            merged_by: b.worker.clone(),
            created_at: now,
        };
        insert_event(
            &t,
            workspace,
            &b.merge_request_id,
            "merge",
            &event,
            now,
            Some(operation),
        )?;
        let state = MergeRequestState::Completed {
            completion: MergeRequestCompletion::Published {
                started_by: started_by.clone(),
                merge_event_id: event.event_id.clone(),
            },
        };
        t.execute("UPDATE merge_requests SET state=?3,updated_at=?4 WHERE workspace_id=?1 AND merge_request_id=?2",
            params![workspace, b.merge_request_id, serde_json::to_string(&state).map_err(|e| MergeRequestError::Operation(e.to_string()))?, now.to_rfc3339()])?;
        ticket_event(&t, &mr, &event, &b.owner_operation_id)?;
        t.commit()?;
        Ok(event)
    }
}

pub(super) fn tables(c: &Connection) -> Result<(), MergeRequestError> {
    c.execute_batch("CREATE TABLE merge_request_publications(
        workspace_id TEXT NOT NULL,operation_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,
        repository_id TEXT NOT NULL,target TEXT NOT NULL,approval_event_id TEXT NOT NULL,
        confirmed INTEGER NOT NULL CHECK(confirmed IN(0,1)),payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
        PRIMARY KEY(workspace_id,operation_id),
        FOREIGN KEY(workspace_id,merge_request_id) REFERENCES merge_requests(workspace_id,merge_request_id));
        CREATE UNIQUE INDEX merge_request_publication_approval ON merge_request_publications(workspace_id,merge_request_id,approval_event_id) WHERE confirmed=0 OR json_extract(payload_json,'$.outcome.kind')='published';
        CREATE UNIQUE INDEX merge_request_publication_target_claim ON merge_request_publications(workspace_id,repository_id,target) WHERE confirmed=0;
        CREATE TRIGGER merge_request_publication_result_immutable BEFORE UPDATE ON merge_request_publications WHEN OLD.confirmed=1 BEGIN SELECT RAISE(ABORT,'publication evidence is immutable'); END;
        CREATE TRIGGER merge_request_publication_no_delete BEFORE DELETE ON merge_request_publications BEGIN SELECT RAISE(ABORT,'publication evidence is retained'); END;")?;
    Ok(())
}

pub(super) fn migrate(c: &Connection) -> Result<(), MergeRequestError> {
    let t = c.unchecked_transaction()?;
    tables(&t)?;
    t.execute(
        "UPDATE merge_request_schema SET version=16 WHERE singleton=1",
        [],
    )?;
    t.commit()?;
    Ok(())
}

pub(super) fn verify(c: &Connection) -> Result<(), MergeRequestError> {
    c.prepare("SELECT workspace_id,operation_id,merge_request_id,repository_id,target,approval_event_id,confirmed,payload_json FROM merge_request_publications LIMIT 0")?;
    Ok(())
}

// Applied v16 DDL above remains frozen. v17 retains the exact historical bytes,
// retires target/approval reservations, and prevents any new receipt writes.
pub(super) fn install_history_only(c: &Connection) -> Result<(), MergeRequestError> {
    c.execute_batch("DROP INDEX merge_request_publication_target_claim;
        DROP INDEX merge_request_publication_approval;
        CREATE TRIGGER merge_request_publication_history_no_insert BEFORE INSERT ON merge_request_publications BEGIN SELECT RAISE(ABORT,'publication history is read-only'); END;
        CREATE TRIGGER merge_request_publication_history_no_update BEFORE UPDATE ON merge_request_publications BEGIN SELECT RAISE(ABORT,'publication history is read-only'); END;")?;
    Ok(())
}

pub(super) fn migrate_history_only(c: &Connection) -> Result<(), MergeRequestError> {
    let t = c.unchecked_transaction()?;
    install_history_only(&t)?;
    t.execute(
        "UPDATE merge_request_schema SET version=17 WHERE singleton=1",
        [],
    )?;
    t.commit()?;
    Ok(())
}
