use super::*;
use crate::restore_checkpoints::{
    RestoreCheckpointAck, RestoreCheckpointCommit, RestoreCheckpointConfig, RestoreCheckpointStage,
    wait_for_release,
};

impl Runtime {
    /// Install an explicitly selected, one-shot process-test gate after normal startup.
    /// It is not persisted or consulted by admission, routing or recovery authority.
    pub fn install_test_restore_checkpoint(
        &self,
        config: RestoreCheckpointConfig,
    ) -> Result<(), RuntimeError> {
        config.validate()?;
        let mut state = self.lock()?;
        if !matches!(state.persistence, RuntimePersistence::Fs(_)) {
            return Err(RuntimeError::InvalidRequest(
                "test restore checkpoint requires an fs Runtime store".into(),
            ));
        }
        state.restore_checkpoint = Some(config);
        Ok(())
    }

    // The lifecycle caller holds the per-Worker operation lock. No Runtime-state
    // mutex is retained while waiting; the real backend candidate remains owned by
    // the same operation throughout this synchronous call.
    pub(super) fn test_restore_checkpoint(
        &self,
        worker_ref: &WorkerRef,
        operation_id: WorkerLifecycleOperationId,
        stage: RestoreCheckpointStage,
    ) {
        let config = {
            let Ok(mut state) = self.lock() else {
                return;
            };
            let slot = &mut state.restore_checkpoint;
            if !slot.as_ref().is_some_and(|config| {
                config.worker_id == worker_ref.worker_id && config.stage == stage
            }) {
                return;
            }
            // Consume even if canonical verification or the handshake fails.
            slot.take().expect("matched checkpoint")
        };
        let Ok(ack) = self.restore_checkpoint_ack(&config, worker_ref, operation_id) else {
            return;
        };
        wait_for_release(&config, &ack);
    }

    pub(super) fn restore_checkpoint_ack(
        &self,
        config: &RestoreCheckpointConfig,
        worker_ref: &WorkerRef,
        operation_id: WorkerLifecycleOperationId,
    ) -> Result<RestoreCheckpointAck, RuntimeError> {
        let invalid = || {
            RuntimeError::InvalidRequest("test restore checkpoint authority did not match".into())
        };
        let state = self.lock()?;
        let RuntimePersistence::Fs(store) = &state.persistence else {
            return Err(invalid());
        };
        // The existing helper validates the exact bounded canonical aggregate and
        // syncs that same file plus its owning directories. Never manufacture a
        // journal marker or infer commit from an in-memory projection.
        let record = store.read_durable_worker_record(worker_ref)?;
        let PersistedWorkerExecutionState::ReconciliationRequired(
            PersistedWorkerLifecycleOperation::Restore(pending),
        ) = &record.execution_state
        else {
            return Err(invalid());
        };
        let expected = match config.stage {
            RestoreCheckpointStage::PreparedNotCommitted => RestoreCommitFact::NotCommitted,
            RestoreCheckpointStage::CommittedPreterminal
            | RestoreCheckpointStage::ActivatedPreterminal => RestoreCommitFact::Committed,
        };
        if config.worker_id != worker_ref.worker_id
            || pending.operation_id != operation_id
            || pending.commit != expected
        {
            return Err(invalid());
        }
        let guard = &record.restore_guard;
        let id = guard.active_request_id.as_ref().ok_or_else(invalid)?;
        let owner = guard.owners.get(id).ok_or_else(invalid)?;
        if owner.operation_id != operation_id
            || owner.request.request_id != *id
            || owner.receipt.is_some()
        {
            return Err(invalid());
        }
        let runtime_id = state.runtime_identity.clone().ok_or_else(invalid)?;
        Ok(RestoreCheckpointAck {
            nonce: config.nonce.clone(),
            stage: config.stage,
            runtime_id,
            worker_id: record.worker_id,
            operation_id,
            commit: match expected {
                RestoreCommitFact::NotCommitted => RestoreCheckpointCommit::NotCommitted,
                RestoreCommitFact::Committed => RestoreCheckpointCommit::Committed,
                RestoreCommitFact::LegacyUnknown => unreachable!(),
            },
            receipt_absent: true,
            pid: std::process::id(),
        })
    }
}
