//! Durable publication claims fence a repository target until a provider result is
//! known. Dispatch is not proof of execution: a lost response retains the claim.
//! Result reconciliation is Backend-owned and does not borrow a former owner's
//! authority to send another push.
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
    /// A dispatch may have reached Runtime. Reconcile the same operation only.
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

    /// Bounded recovery page; no rows here constitute permission to send a push.
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

    /// Returns true only for the caller that atomically acquired the dispatch
    /// claim. Exact replay returns false and MUST NOT dispatch a second push.
    /// Admission runs under the writer transaction and must not call providers.
    pub fn claim_publication(
        &self,
        binding: PublicationBinding,
        now: DateTime<Utc>,
        admit: impl Fn(&Connection) -> Result<(), MergeRequestError>,
    ) -> Result<(Publication, bool), MergeRequestError> {
        for (name, value) in [
            ("operation_id", &binding.operation_id),
            ("working_directory_id", &binding.working_directory_id),
            ("connection_id", &binding.connection_id),
            ("source", &binding.source),
            ("target_before", &binding.target_before),
        ] {
            nonempty(name, value)?;
        }
        if !binding.target.starts_with("refs/heads/") || binding.target == "refs/heads/" {
            return Err(MergeRequestError::Validation(
                "publication target must be a canonical branch".into(),
            ));
        }
        let mut c = self.lock()?;
        let t = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        admit(&t)?;
        if let Some(saved) = load(&t, &binding.workspace_id, &binding.operation_id)? {
            if saved.binding != binding {
                return Err(MergeRequestError::Conflict(
                    "publication fingerprint mismatch".into(),
                ));
            }
            return Ok((saved, false));
        }
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
        let busy: bool = t.query_row("SELECT EXISTS(SELECT 1 FROM merge_request_publications WHERE workspace_id=?1 AND repository_id=?2 AND target=?3 AND confirmed=0)",
            params![binding.workspace_id, binding.repository_id, binding.target], |r| r.get(0))?;
        if busy {
            return Err(MergeRequestError::Conflict(
                "target publication is awaiting a provider result".into(),
            ));
        }
        let already: bool = t.query_row("SELECT EXISTS(SELECT 1 FROM merge_request_publications WHERE workspace_id=?1 AND merge_request_id=?2 AND approval_event_id=?3 AND json_extract(payload_json,'$.outcome.kind') != 'not_published')",
            params![binding.workspace_id, binding.merge_request_id, binding.approval_event_id], |r| r.get(0))?;
        if already {
            return Err(MergeRequestError::Conflict(
                "this approval already has a publication operation; reconcile its result".into(),
            ));
        }
        let publication = Publication {
            binding,
            ticket_content_digest: digest,
            outcome: PublicationOutcome::Unconfirmed {},
            created_at: now.to_rfc3339(),
        };
        t.execute("INSERT INTO merge_request_publications(workspace_id,operation_id,merge_request_id,repository_id,target,approval_event_id,confirmed,payload_json) VALUES(?1,?2,?3,?4,?5,?6,0,?7)",
            params![publication.binding.workspace_id, publication.binding.operation_id, publication.binding.merge_request_id,
                publication.binding.repository_id, publication.binding.target, publication.binding.approval_event_id, serde_json::to_string(&publication).map_err(|e| MergeRequestError::Operation(e.to_string()))?])?;
        t.commit()?;
        Ok((publication, true))
    }

    /// Record only a trusted Runtime receipt matching the entire saved binding.
    /// This is not a model-facing assertion of publication. Owner/policy changes
    /// do not erase a sent operation's facts. A contradictory receipt is rejected.
    pub fn record_publication_outcome(
        &self,
        binding: &PublicationBinding,
        outcome: PublicationOutcome,
    ) -> Result<Publication, MergeRequestError> {
        match &outcome {
            PublicationOutcome::Unconfirmed {} => {
                return self
                    .publication(&binding.workspace_id, &binding.operation_id)?
                    .ok_or(MergeRequestError::NotFound);
            }
            PublicationOutcome::Published {
                target_after,
                observed_at,
            } => {
                if target_after != &binding.source {
                    return Err(MergeRequestError::Conflict(
                        "publication result is not the approved source".into(),
                    ));
                }
                nonempty("provider observation time", observed_at)?;
            }
            PublicationOutcome::NotPublished { reason } => {
                bounded_body("publication reason", reason)?
            }
        }
        let mut c = self.lock()?;
        let t = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut publication = load(&t, &binding.workspace_id, &binding.operation_id)?
            .ok_or(MergeRequestError::NotFound)?;
        if &publication.binding != binding {
            return Err(MergeRequestError::Conflict(
                "provider receipt binding mismatch".into(),
            ));
        }
        if !matches!(publication.outcome, PublicationOutcome::Unconfirmed {}) {
            if publication.outcome == outcome {
                return Ok(publication);
            }
            return Err(MergeRequestError::Conflict(
                "publication result is immutable".into(),
            ));
        }
        publication.outcome = outcome;
        t.execute("UPDATE merge_request_publications SET confirmed=1,payload_json=?3 WHERE workspace_id=?1 AND operation_id=?2 AND confirmed=0",
            params![binding.workspace_id, binding.operation_id, serde_json::to_string(&publication).map_err(|e| MergeRequestError::Operation(e.to_string()))?])?;
        t.commit()?;
        Ok(publication)
    }

    /// Complete from immutable publication evidence, not current target position
    /// or a Subject's adoption stamp. Canceled MRs and changed requirements retain
    /// the publication fact without being silently concluded.
    pub fn complete_publication(
        &self,
        workspace: &str,
        operation: &str,
        now: DateTime<Utc>,
    ) -> Result<MergeEvent, MergeRequestError> {
        let mut c = self.lock()?;
        let t = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let publication = load(&t, workspace, operation)?.ok_or(MergeRequestError::NotFound)?;
        let b = &publication.binding;
        let PublicationOutcome::Published { target_after, .. } = &publication.outcome else {
            return Err(MergeRequestError::NotReady(
                "provider-confirmed publication is required".into(),
            ));
        };
        let mr = load_mr(&t, workspace, &b.merge_request_id)?.ok_or(MergeRequestError::NotFound)?;
        if let Some(event) = mr.thread.iter().find_map(|e| match e {
            MergeRequestThreadEvent::Merge(e) if e.operation_id == operation => Some(e),
            _ => None,
        }) {
            return Ok(event.clone());
        }
        let MergeRequestState::Integration { started_by } = &mr.state else {
            return Err(MergeRequestError::NotReady(
                "publication is retained but MR is no longer in integration".into(),
            ));
        };
        let ticket = mr.ticket_ids.first().ok_or(MergeRequestError::NotFound)?;
        if current_ticket_content_digest(&t, workspace, ticket)?.as_ref()
            != Some(&publication.ticket_content_digest)
            || !mr.effective_review(&b.source).is_some_and(|r| {
                r.event_id == b.approval_event_id && r.decision == ReviewDecision::Approve
            })
        {
            return Err(MergeRequestError::NotReady(
                "publication is retained but requirements or approval changed".into(),
            ));
        }
        // Existing issued review capabilities may not create a later verdict on
        // a source that has already been published by this operation.
        t.execute("UPDATE merge_request_review_grants SET status='revoked',revoked_at=?3 WHERE workspace_id=?1 AND merge_request_id=?2 AND status='issued'",
            params![workspace, b.merge_request_id, now.to_rfc3339()])?;
        let event = MergeEvent {
            event_id: Uuid::now_v7().to_string(),
            sequence: next_seq(&t, workspace, &b.merge_request_id)?,
            operation_id: operation.into(),
            approval_event_id: b.approval_event_id.clone(),
            approved_source_ref: b.source.clone(),
            target_ref_before: b.target_before.clone(),
            target_ref_after: target_after.clone(),
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
