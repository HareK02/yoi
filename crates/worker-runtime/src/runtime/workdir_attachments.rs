use super::*;
use runtime_api::{
    WorkerWorkdirAttachmentsOperationObservation as OperationObservation,
    WorkerWorkdirAttachmentsOperationOutcome as Outcome,
    WorkerWorkdirAttachmentsOperationReceipt as Receipt,
    WorkerWorkdirAttachmentsOperationRequest as Request,
};

/// Attachment authority has its own incarnation fence, independent of execution
/// state and Restore observations. Owners survive supersession for this aggregate's
/// lifetime; the canonical aggregate size limit provides replay-history backpressure.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkdirAttachmentsGuard {
    pub(super) generation: String,
    owners: BTreeMap<String, WorkdirAttachmentsOwner>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkdirAttachmentsOwner {
    workspace_id: Option<String>,
    receipt: Receipt,
}

impl Default for WorkdirAttachmentsGuard {
    fn default() -> Self {
        Self {
            generation: Uuid::now_v7().to_string(),
            owners: BTreeMap::new(),
        }
    }
}

impl WorkdirAttachmentsGuard {
    pub(super) fn rotate(&mut self) {
        self.generation = Uuid::now_v7().to_string();
    }

    pub(crate) fn validate(
        &self,
        worker_ref: &WorkerRef,
        workspace_id: &Option<String>,
    ) -> Result<(), RuntimeError> {
        if self.generation.trim().is_empty() {
            return Err(RuntimeError::InvalidRequest(
                "missing attachment generation".into(),
            ));
        }
        for (id, owner) in &self.owners {
            validate_request(&owner.receipt.request)?;
            if id != &owner.receipt.request.request_id
                || owner.workspace_id != *workspace_id
                || owner.receipt.worker_ref.worker_id != worker_ref.worker_id
                || owner.receipt.resulting_generation.trim().is_empty()
                || owner.receipt.resulting_generation == owner.receipt.request.expected_generation
            {
                return Err(RuntimeError::InvalidRequest(
                    "invalid attachment operation owner".into(),
                ));
            }
        }
        Ok(())
    }

    fn outcome(
        &self,
        worker_ref: &WorkerRef,
        workspace_id: &Option<String>,
        request: &Request,
    ) -> Outcome {
        match self.owners.get(&request.request_id) {
            None => Outcome::Absent,
            Some(owner)
                if owner.workspace_id == *workspace_id
                    && owner.receipt.worker_ref.worker_id == worker_ref.worker_id
                    && owner.receipt.request == *request =>
            {
                Outcome::Receipt {
                    receipt: owner.receipt.clone(),
                }
            }
            Some(_) => Outcome::Conflict,
        }
    }
}

fn validate_request_identity(request: &Request) -> Result<(), RuntimeError> {
    if request.request_id.trim().is_empty() || request.expected_generation.trim().is_empty() {
        return Err(RuntimeError::InvalidRequest(
            "attachment operation requires requestId and expectedGeneration".into(),
        ));
    }
    Ok(())
}

fn validate_request(request: &Request) -> Result<Vec<LogicalWorkdirAttachment>, RuntimeError> {
    validate_request_identity(request)?;
    let attachments = request
        .logical_workdir_attachments
        .iter()
        .map(|attachment| LogicalWorkdirAttachment {
            alias: attachment.alias.clone(),
            working_directory_id: attachment.working_directory_id.clone(),
            capabilities: attachment.capabilities,
        })
        .collect::<Vec<_>>();
    validate_logical_workdir_attachments(&attachments)?;
    Ok(attachments)
}

impl RuntimeState {
    // In particular, a query must not adopt an unclaimed Workspace owner.
    pub(super) fn check_attachment_scope(
        &mut self,
        worker_ref: &WorkerRef,
        scope: Option<&RuntimeWorkspaceScope>,
    ) -> Result<(), RuntimeError> {
        if let Some(scope) = scope {
            if !self
                .worker(worker_ref)?
                .belongs_to_workspace(&scope.workspace_id)
                || !self.ensure_workspace_owner(scope, false)?
            {
                return Err(RuntimeError::WorkerNotFound {
                    worker_id: worker_ref.worker_id,
                });
            }
        }
        Ok(())
    }
}

impl Runtime {
    pub fn apply_worker_workdir_attachments_operation(
        &self,
        worker_ref: &WorkerRef,
        request: Request,
    ) -> Result<Receipt, RuntimeError> {
        self.apply_worker_workdir_attachments_operation_with_scope(worker_ref, request, None)
    }

    pub fn apply_worker_workdir_attachments_operation_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
        request: Request,
    ) -> Result<Receipt, RuntimeError> {
        self.apply_worker_workdir_attachments_operation_with_scope(worker_ref, request, Some(scope))
    }

    fn apply_worker_workdir_attachments_operation_with_scope(
        &self,
        worker_ref: &WorkerRef,
        request: Request,
        scope: Option<&RuntimeWorkspaceScope>,
    ) -> Result<Receipt, RuntimeError> {
        let operation_lock = self.worker_operation_lock(worker_ref.worker_id)?;
        let _operation_guard = operation_lock
            .lock()
            .map_err(|_| RuntimeError::StatePoisoned)?;
        validate_request_identity(&request)?;
        let mut state = self.lock()?;
        state.check_attachment_scope(worker_ref, scope)?;
        state.refresh_uncertain_worker_write(worker_ref)?;
        state.check_attachment_scope(worker_ref, scope)?;
        let worker = state.worker(worker_ref)?;
        match worker
            .workdir_attachments_guard
            .outcome(worker_ref, &worker.workspace_id, &request)
        {
            Outcome::Receipt { receipt } => return Ok(receipt),
            Outcome::Conflict => {
                return Err(RuntimeError::WorkdirAttachmentsRequestConflict {
                    worker_id: worker_ref.worker_id,
                });
            }
            _ => {}
        }
        if worker.workdir_attachments_guard.generation != request.expected_generation {
            return Err(RuntimeError::WorkdirAttachmentsGenerationStale {
                worker_id: worker_ref.worker_id,
            });
        }
        if worker.has_pending_lifecycle_operation() {
            return Err(RuntimeError::WorkdirAttachmentsRequestConflict {
                worker_id: worker_ref.worker_id,
            });
        }
        state.ensure_running()?;
        let attachments = validate_request(&request)?;
        for attachment in &attachments {
            state.ensure_workdir_not_removing(&attachment.working_directory_id)?;
        }
        let mut candidate = state.worker(worker_ref)?.clone();
        candidate.apply_logical_workdir_attachments(attachments)?;
        let receipt = Receipt {
            worker_ref: runtime_api::WorkerRef {
                worker_id: worker_ref.worker_id,
            },
            request,
            resulting_generation: candidate.workdir_attachments_guard.generation.clone(),
        };
        candidate.workdir_attachments_guard.owners.insert(
            receipt.request.request_id.clone(),
            WorkdirAttachmentsOwner {
                workspace_id: candidate.workspace_id.clone(),
                receipt: receipt.clone(),
            },
        );
        // One canonical save contains the owner, receipt, desired set, generation,
        // and pruned local/Restore claims. No receipt is synthesized by set equality.
        state.persist_worker_record_with_fence(&candidate)?;
        state.workers.insert(worker_ref.worker_id, candidate);
        state.publish_worker_upsert(worker_ref.worker_id)?;
        Ok(receipt)
    }

    pub fn observe_worker_workdir_attachments_operation(
        &self,
        worker_ref: &WorkerRef,
        request: Request,
    ) -> Result<OperationObservation, RuntimeError> {
        self.observe_worker_workdir_attachments_operation_with_scope(worker_ref, request, None)
    }

    pub fn observe_worker_workdir_attachments_operation_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
        request: Request,
    ) -> Result<OperationObservation, RuntimeError> {
        self.observe_worker_workdir_attachments_operation_with_scope(
            worker_ref,
            request,
            Some(scope),
        )
    }

    fn observe_worker_workdir_attachments_operation_with_scope(
        &self,
        worker_ref: &WorkerRef,
        request: Request,
        scope: Option<&RuntimeWorkspaceScope>,
    ) -> Result<OperationObservation, RuntimeError> {
        let operation_lock = self.worker_operation_lock(worker_ref.worker_id)?;
        let _operation_guard = operation_lock
            .lock()
            .map_err(|_| RuntimeError::StatePoisoned)?;
        validate_request_identity(&request)?;
        let mut state = self.lock()?;
        state.check_attachment_scope(worker_ref, scope)?;
        let worker = state.worker(worker_ref)?;
        let mut result =
            worker
                .workdir_attachments_guard
                .outcome(worker_ref, &worker.workspace_id, &request);
        let mut current_generation = Some(worker.workdir_attachments_guard.generation.clone());
        #[cfg(feature = "fs-store")]
        if let Some(store) = state.fs_store() {
            // Read/sync only. Never refresh/adopt canonical memory, reserve an
            // owner, resend a request, or compensate a superseded operation.
            let (observed, consistent) =
                store.reconcile_worker_record(&worker.persisted_record())?;
            if observed.workspace_id != worker.workspace_id {
                result = Outcome::Unconfirmed;
                current_generation = None;
            } else {
                result = observed.workdir_attachments_guard.outcome(
                    worker_ref,
                    &observed.workspace_id,
                    &request,
                );
                if !consistent {
                    current_generation = None;
                }
            }
        }
        Ok(OperationObservation {
            worker_ref: runtime_api::WorkerRef {
                worker_id: worker_ref.worker_id,
            },
            request,
            result,
            current_generation,
        })
    }

    pub fn observe_worker_workdir_attachments_state(
        &self,
        worker_ref: &WorkerRef,
        attachments: Vec<LogicalWorkdirAttachment>,
    ) -> Result<runtime_api::WorkerWorkdirAttachmentsObservation, RuntimeError> {
        self.observe_worker_workdir_attachments_with_scope(worker_ref, attachments, None)
    }

    pub fn observe_worker_workdir_attachments_state_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        worker_ref: &WorkerRef,
        attachments: Vec<LogicalWorkdirAttachment>,
    ) -> Result<runtime_api::WorkerWorkdirAttachmentsObservation, RuntimeError> {
        self.observe_worker_workdir_attachments_with_scope(worker_ref, attachments, Some(scope))
    }
}
