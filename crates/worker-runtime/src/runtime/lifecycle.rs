/// Runtime owns lifecycle decisions and projections, not execution handles or tasks.
/// A saved runnable specification is deliberately not a live execution.
#[derive(Debug, Clone)]
enum WorkerExecutionState {
    NotStarted,
    AwaitingRestore(WorkerStatus),
    Active(ActiveWorkerExecution),
    Stopped,
    Creating(CreateOperation),
    Restoring(RestoreOperation),
    Stopping(StopOperation),
    RecoveryRequired(WorkerLifecycleOperation),
}

#[derive(Debug, Clone)]
struct ActiveWorkerExecution {
    snapshot: protocol::WorkerStateSnapshot,
    owner: Option<WorkerLifecycleOperationId>,
    observation_valid: bool,
    internal_workers: InternalWorkerActivityProjection,
}

impl ActiveWorkerExecution {
    fn new(snapshot: protocol::WorkerStateSnapshot) -> Self {
        Self {
            snapshot,
            owner: None,
            observation_valid: true,
            internal_workers: InternalWorkerActivityProjection::default(),
        }
    }

    fn status(&self) -> WorkerStatus {
        match self.snapshot.catalog_status() {
            protocol::WorkerStatus::Idle => WorkerStatus::Idle,
            protocol::WorkerStatus::Running => WorkerStatus::Running,
            protocol::WorkerStatus::Paused => WorkerStatus::Paused,
            // A protocol terminal observation is not the owner-side join and
            // cleanup decision. Keep durable runnable intent until common Stop.
            protocol::WorkerStatus::Stopped => WorkerStatus::Idle,
        }
    }
}

#[derive(Debug, Clone)]
struct CreateOperation {
    // Legacy unresolved creates have no operation identity. Never invent one on reopen.
    operation_id: Option<WorkerLifecycleOperationId>,
    snapshot: Option<protocol::WorkerStateSnapshot>,
    observation_valid: bool,
    internal_workers: InternalWorkerActivityProjection,
}

#[derive(Debug, Clone)]
struct RestoreOperation {
    pending: PendingWorkerRestore,
    candidate_context: Option<crate::execution::WorkerExecutionContext>,
    candidate: RestoreCandidate,
}

#[derive(Debug, Clone)]
enum RestoreCandidate {
    Unprepared,
    Existing(ActiveWorkerExecution),
    Prepared(ActiveWorkerExecution),
    Publishing(ActiveWorkerExecution),
}

impl RestoreCandidate {
    fn execution(&self) -> Option<&ActiveWorkerExecution> {
        match self {
            Self::Unprepared => None,
            Self::Existing(active) | Self::Prepared(active) | Self::Publishing(active) => {
                Some(active)
            }
        }
    }
}

#[derive(Debug, Clone)]
enum StopProgress {
    AwaitingTermination,
    ScopeTerminated(crate::execution::WorkerExecutionTermination),
}

#[derive(Debug, Clone)]
struct StopOperation {
    pending: PendingWorkerStop,
    progress: StopProgress,
    restore: Option<RestoreOperation>,
    #[cfg(feature = "fs-store")]
    restore_finalization: Option<RestoreFinalization>,
    create: Option<CreateOperation>,
    execution: Option<ActiveWorkerExecution>,
}

#[derive(Debug, Clone)]
enum WorkerLifecycleOperation {
    Create(CreateOperation),
    Restore(RestoreOperation),
    Stop(StopOperation),
    #[cfg(feature = "fs-store")]
    RestoreFinalization(RestoreFinalization),
}

#[cfg(feature = "fs-store")]
#[derive(Debug, Clone)]
struct RestoreFinalization {
    operation_id: WorkerLifecycleOperationId,
    evidence: RestoreFinalizationEvidence,
    last_settled_status: WorkerStatus,
}

impl WorkerRecord {
    fn lifecycle(&self) -> crate::catalog::WorkerLifecycleState {
        use crate::catalog::WorkerLifecycleState as Projection;
        match &self.execution {
            WorkerExecutionState::NotStarted => Projection::NotStarted,
            WorkerExecutionState::AwaitingRestore(_) => Projection::AwaitingRestore,
            WorkerExecutionState::Active(_) => Projection::Active,
            WorkerExecutionState::Stopped => Projection::Stopped,
            WorkerExecutionState::Creating(_) => Projection::Creating,
            WorkerExecutionState::Restoring(_) => Projection::Restoring,
            WorkerExecutionState::Stopping(_) => Projection::Stopping,
            WorkerExecutionState::RecoveryRequired(_) => Projection::RecoveryRequired,
        }
    }

    fn status(&self) -> WorkerStatus {
        match &self.execution {
            WorkerExecutionState::NotStarted | WorkerExecutionState::Stopped => {
                WorkerStatus::Stopped
            }
            WorkerExecutionState::AwaitingRestore(status) => *status,
            WorkerExecutionState::Active(active) => active.status(),
            WorkerExecutionState::Creating(_)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Create(_)) => {
                WorkerStatus::Idle
            }
            WorkerExecutionState::Restoring(restore)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Restore(restore)) => {
                restore.pending.last_settled_status
            }
            WorkerExecutionState::Stopping(stop)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Stop(stop)) => {
                stop.pending.last_settled_status
            }
            #[cfg(feature = "fs-store")]
            WorkerExecutionState::RecoveryRequired(
                WorkerLifecycleOperation::RestoreFinalization(finalization),
            ) => finalization.last_settled_status,
        }
    }

    fn active_execution(&self) -> Option<&ActiveWorkerExecution> {
        match &self.execution {
            WorkerExecutionState::Active(active) => Some(active),
            _ => None,
        }
    }

    fn active_execution_mut(&mut self) -> Option<&mut ActiveWorkerExecution> {
        match &mut self.execution {
            WorkerExecutionState::Active(active) => Some(active),
            _ => None,
        }
    }

    fn observation_state(&self) -> crate::catalog::WorkerObservationState {
        match self.active_execution() {
            Some(active) if active.observation_valid => {
                crate::catalog::WorkerObservationState::Current {
                    state_sequence: active.snapshot.state_sequence,
                }
            }
            _ => crate::catalog::WorkerObservationState::Unavailable,
        }
    }

    fn worker_state(&self) -> Option<protocol::WorkerStateSnapshot> {
        self.observation_execution()
            .filter(|active| active.observation_valid)
            .map(|active| active.snapshot.clone())
    }

    fn execution_metadata_available(&self) -> bool {
        self.request.is_some()
    }

    fn is_settled_inactive(&self) -> bool {
        matches!(
            self.execution,
            WorkerExecutionState::Stopped | WorkerExecutionState::NotStarted
        )
    }

    fn awaiting_restore(&self) -> bool {
        matches!(self.execution, WorkerExecutionState::AwaitingRestore(_))
    }

    fn restore_operation(&self) -> Option<&RestoreOperation> {
        match &self.execution {
            WorkerExecutionState::Restoring(restore)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Restore(restore)) => {
                Some(restore)
            }
            WorkerExecutionState::Stopping(stop)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Stop(stop)) => {
                stop.restore.as_ref()
            }
            _ => None,
        }
    }

    fn restore_operation_mut(&mut self) -> Option<&mut RestoreOperation> {
        match &mut self.execution {
            WorkerExecutionState::Restoring(restore)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Restore(restore)) => {
                Some(restore)
            }
            WorkerExecutionState::Stopping(stop)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Stop(stop)) => {
                stop.restore.as_mut()
            }
            _ => None,
        }
    }

    fn pending_restore(&self) -> Option<PendingWorkerRestore> {
        self.restore_operation().map(|restore| restore.pending)
    }
    fn pending_restore_mut(&mut self) -> Option<&mut PendingWorkerRestore> {
        self.restore_operation_mut()
            .map(|restore| &mut restore.pending)
    }
    fn stop_operation(&self) -> Option<&StopOperation> {
        match &self.execution {
            WorkerExecutionState::Stopping(stop)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Stop(stop)) => {
                Some(stop)
            }
            _ => None,
        }
    }
    fn stop_operation_mut(&mut self) -> Option<&mut StopOperation> {
        match &mut self.execution {
            WorkerExecutionState::Stopping(stop)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Stop(stop)) => {
                Some(stop)
            }
            _ => None,
        }
    }
    fn pending_stop(&self) -> Option<PendingWorkerStop> {
        self.stop_operation().map(|stop| stop.pending)
    }
    fn stop_termination(&self) -> Option<&crate::execution::WorkerExecutionTermination> {
        match &self.stop_operation()?.progress {
            StopProgress::AwaitingTermination => None,
            StopProgress::ScopeTerminated(termination) => Some(termination),
        }
    }
    fn restore_candidate_context(&self) -> Option<crate::execution::WorkerExecutionContext> {
        self.restore_operation()?.candidate_context.clone()
    }
    fn take_restore_candidate_context(
        &mut self,
    ) -> Option<crate::execution::WorkerExecutionContext> {
        self.restore_operation_mut()?.candidate_context.take()
    }
    fn retain_restore_candidate_context(
        &mut self,
        context: crate::execution::WorkerExecutionContext,
    ) -> Result<(), RuntimeError> {
        let restore = self.restore_operation_mut().ok_or_else(|| {
            RuntimeError::InvalidRequest("Restore candidate has no lifecycle owner".into())
        })?;
        restore.candidate_context = Some(context);
        Ok(())
    }
    fn begin_restore(
        &mut self,
        pending: PendingWorkerRestore,
        context: Option<crate::execution::WorkerExecutionContext>,
    ) {
        let candidate = self
            .restore_operation()
            .map(|restore| restore.candidate.clone())
            .or_else(|| {
                self.active_execution()
                    .cloned()
                    .map(RestoreCandidate::Existing)
            })
            .unwrap_or(RestoreCandidate::Unprepared);
        let candidate = if context.is_some() && matches!(candidate, RestoreCandidate::Existing(_)) {
            RestoreCandidate::Unprepared
        } else {
            candidate
        };
        self.execution = WorkerExecutionState::Restoring(RestoreOperation {
            pending,
            candidate_context: context,
            candidate,
        });
    }
    fn begin_stop(&mut self, pending: PendingWorkerStop) {
        #[cfg(feature = "fs-store")]
        let restore_finalization = match &self.execution {
            WorkerExecutionState::RecoveryRequired(
                WorkerLifecycleOperation::RestoreFinalization(finalization),
            ) => Some(finalization.clone()),
            _ => None,
        };
        let old = std::mem::replace(&mut self.execution, WorkerExecutionState::NotStarted);
        let (restore, create, execution) = match old {
            WorkerExecutionState::Restoring(restore)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Restore(restore)) => {
                (Some(restore), None, None)
            }
            WorkerExecutionState::Creating(create)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Create(create)) => {
                (None, Some(create), None)
            }
            WorkerExecutionState::Active(active) => (None, None, Some(active)),
            _ => (None, None, None),
        };
        self.execution = WorkerExecutionState::Stopping(StopOperation {
            pending,
            progress: StopProgress::AwaitingTermination,
            restore,
            #[cfg(feature = "fs-store")]
            restore_finalization,
            create,
            execution,
        });
    }
    /// Only operation owners call this after their execution/cleanup has finished.
    fn settle_stopped(&mut self) {
        self.execution = WorkerExecutionState::Stopped;
    }

    fn settle_restore(&mut self) {
        #[cfg(feature = "fs-store")]
        if let WorkerExecutionState::RecoveryRequired(
            WorkerLifecycleOperation::RestoreFinalization(finalization),
        ) = &self.execution
        {
            self.execution = if finalization.last_settled_status.is_active() {
                WorkerExecutionState::AwaitingRestore(finalization.last_settled_status)
            } else {
                WorkerExecutionState::Stopped
            };
            return;
        }
        let Some(restore) = self.restore_operation().cloned() else {
            return;
        };
        self.execution = match restore.candidate {
            RestoreCandidate::Existing(active) => WorkerExecutionState::Active(active),
            RestoreCandidate::Publishing(active)
                if restore.pending.commit == RestoreCommitFact::Committed =>
            {
                WorkerExecutionState::Active(active)
            }
            _ if restore.pending.last_settled_status.is_active() => {
                WorkerExecutionState::AwaitingRestore(restore.pending.last_settled_status)
            }
            _ => WorkerExecutionState::Stopped,
        };
    }

    fn handoff_active(
        &mut self,
        mut snapshot: protocol::WorkerStateSnapshot,
    ) -> Result<(), RuntimeError> {
        let (owner, observation_valid, internal_workers) = match &self.execution {
            WorkerExecutionState::Creating(create)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Create(create)) => {
                if let Some(latest) = &create.snapshot {
                    if latest.state_sequence == snapshot.state_sequence && latest != &snapshot {
                        return Err(RuntimeError::InvalidRequest(
                            "conflicting Create snapshot source sequence".into(),
                        ));
                    }
                    if latest.state_sequence > snapshot.state_sequence {
                        snapshot = latest.clone();
                    }
                }
                (
                    create.operation_id,
                    create.observation_valid,
                    create.internal_workers.clone(),
                )
            }
            _ => (None, false, InternalWorkerActivityProjection::default()),
        };
        self.remember_finished_submission(&snapshot);
        self.execution = WorkerExecutionState::Active(ActiveWorkerExecution {
            snapshot,
            owner,
            observation_valid,
            internal_workers,
        });
        Ok(())
    }

    fn remember_finished_submission(&mut self, snapshot: &protocol::WorkerStateSnapshot) {
        if let Some(request_id) = &snapshot.last_finished_submission_request_id {
            self.last_finished_submission_request_id = Some(request_id.clone());
        }
    }

    fn retain_candidate_snapshot(&mut self, snapshot: protocol::WorkerStateSnapshot) {
        self.remember_finished_submission(&snapshot);
        match &mut self.execution {
            WorkerExecutionState::Creating(create)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Create(create)) => {
                if create
                    .snapshot
                    .as_ref()
                    .is_none_or(|old| snapshot.state_sequence > old.state_sequence)
                {
                    create.snapshot = Some(snapshot);
                    create.observation_valid = true;
                }
            }
            WorkerExecutionState::Restoring(restore)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Restore(restore)) => {
                let mut active = ActiveWorkerExecution::new(snapshot);
                active.owner = Some(restore.pending.operation_id);
                restore.candidate = RestoreCandidate::Prepared(active)
            }
            _ => {}
        }
    }

    fn invalidate_active(&mut self) {
        if let WorkerExecutionState::Active(active) = &self.execution {
            self.execution = WorkerExecutionState::AwaitingRestore(active.status());
            self.restore_guard.rotate();
        }
    }
}

#[cfg(feature = "fs-store")]
impl RestoreOperation {
    fn from_persisted(restore: PersistedWorkerRestoreOperation) -> Self {
        Self {
            pending: PendingWorkerRestore {
                operation_id: restore.operation_id,
                mode: restore.mode.into(),
                commit: restore.commit,
                source_repair_possible: restore.source_repair_possible,
                last_settled_status: restore.last_settled_status,
            },
            candidate_context: None,
            candidate: RestoreCandidate::Unprepared,
        }
    }
    fn persisted(&self, worker: &WorkerRecord) -> PersistedWorkerRestoreOperation {
        PersistedWorkerRestoreOperation {
            operation_id: self.pending.operation_id,
            mode: self.pending.mode.into(),
            commit: self.pending.commit,
            source_repair_possible: self.pending.source_repair_possible,
            request: worker
                .request
                .clone()
                .expect("Restore retains specification"),
            binding: worker
                .execution_bound
                .then_some(PersistedWorkerExecutionBinding {}),
            last_settled_status: self.pending.last_settled_status,
        }
    }
}

impl WorkerRecord {
    fn retain_live_projection(&mut self, previous: &Self) {
        match (&mut self.execution, &previous.execution) {
            (
                WorkerExecutionState::AwaitingRestore(_),
                WorkerExecutionState::Restoring(restore)
                | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Restore(restore)),
            ) if restore.pending.commit == RestoreCommitFact::Committed
                && self.restore_guard.owners.values().any(|owner| {
                    owner.operation_id == restore.pending.operation_id
                        && owner
                            .receipt
                            .as_ref()
                            .is_some_and(|receipt| receipt.state == WorkerRestoreState::Accepted)
                }) =>
            {
                if let RestoreCandidate::Publishing(active) = &restore.candidate {
                    self.execution = WorkerExecutionState::Active(active.clone());
                }
            }
            (WorkerExecutionState::AwaitingRestore(_), WorkerExecutionState::Active(active)) => {
                self.execution = WorkerExecutionState::Active(active.clone());
            }
            (
                WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Create(current)),
                WorkerExecutionState::Creating(old)
                | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Create(old)),
            ) if current.operation_id == old.operation_id => {
                current.snapshot = old.snapshot.clone();
                current.observation_valid = old.observation_valid;
                current.internal_workers = old.internal_workers.clone();
            }
            (
                WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Stop(current)),
                WorkerExecutionState::Stopping(old)
                | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Stop(old)),
            ) if current.pending.operation_id == old.pending.operation_id => {
                if matches!(current.progress, StopProgress::AwaitingTermination) {
                    current.progress = old.progress.clone();
                }
                current.execution = old.execution.clone();
                current.create = old.create.clone();
            }
            _ => {}
        }
        let old_restore = previous.restore_operation();
        if let (Some(current), Some(old)) = (self.restore_operation_mut(), old_restore) {
            if current.pending.operation_id == old.pending.operation_id {
                current.candidate_context = old.candidate_context.clone();
                current.candidate = old.candidate.clone();
            }
        }
    }
}

impl WorkerRecord {
    fn require_create_recovery(&mut self) {
        let operation = match &self.execution {
            WorkerExecutionState::Creating(create)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Create(create)) => {
                create.clone()
            }
            _ => CreateOperation {
                operation_id: None,
                snapshot: None,
                observation_valid: false,
                internal_workers: InternalWorkerActivityProjection::default(),
            },
        };
        self.execution =
            WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Create(operation));
    }
}

impl WorkerRecord {
    // Only an execution owner may start publication, after aggregate commit and
    // backend activation. Buffered events still belong to Restore until receipt.
    fn begin_restore_publication(&mut self) -> Result<(), RuntimeError> {
        let restore = self.restore_operation_mut().ok_or_else(|| {
            RuntimeError::InvalidRequest("Restore publication has no lifecycle owner".into())
        })?;
        if restore.pending.commit != RestoreCommitFact::Committed {
            return Err(RuntimeError::InvalidRequest(
                "Restore publication precedes aggregate commit".into(),
            ));
        }
        restore.candidate = match &restore.candidate {
            RestoreCandidate::Prepared(active) | RestoreCandidate::Publishing(active) => {
                RestoreCandidate::Publishing(active.clone())
            }
            RestoreCandidate::Existing(_) | RestoreCandidate::Unprepared => {
                return Err(RuntimeError::InvalidRequest(
                    "Restore publication has no candidate snapshot".into(),
                ));
            }
        };
        Ok(())
    }

    fn observation_execution(&self) -> Option<&ActiveWorkerExecution> {
        self.active_execution()
            .or_else(|| match &self.restore_operation()?.candidate {
                RestoreCandidate::Publishing(active) => Some(active),
                _ => None,
            })
    }

    fn observation_execution_mut(&mut self) -> Option<&mut ActiveWorkerExecution> {
        match &mut self.execution {
            WorkerExecutionState::Active(active) => Some(active),
            WorkerExecutionState::Restoring(restore)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Restore(restore)) => {
                match &mut restore.candidate {
                    RestoreCandidate::Publishing(active) => Some(active),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn accepts_observation_owner(&self, owner: WorkerLifecycleOperationId) -> bool {
        if self
            .observation_execution()
            .is_some_and(|active| active.owner == Some(owner))
        {
            return true;
        }
        matches!(&self.execution, WorkerExecutionState::Creating(create)
            if create.operation_id == Some(owner))
    }

    fn owns_observation_context(&self, owner: WorkerLifecycleOperationId) -> bool {
        self.accepts_observation_owner(owner)
            || self
                .restore_operation()
                .is_some_and(|restore| restore.pending.operation_id == owner)
    }

    fn invalidate_observation(&mut self) {
        if let Some(active) = self.observation_execution_mut() {
            active.observation_valid = false;
        }
        match &mut self.execution {
            WorkerExecutionState::Creating(create)
            | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Create(create)) => {
                create.observation_valid = false;
            }
            WorkerExecutionState::Restoring(restore) => match &mut restore.candidate {
                RestoreCandidate::Existing(active)
                | RestoreCandidate::Prepared(active)
                | RestoreCandidate::Publishing(active) => active.observation_valid = false,
                _ => {}
            },
            _ => {}
        }
    }

    fn apply_owned_observation(&mut self, snapshot: &protocol::WorkerStateSnapshot) -> bool {
        if self.active_execution().is_some() {
            return self.apply_worker_state(snapshot);
        }
        if self.observation_execution().is_none() {
            if let WorkerExecutionState::Creating(create) = &self.execution {
                if create
                    .snapshot
                    .as_ref()
                    .is_some_and(|old| old.state_sequence >= snapshot.state_sequence)
                {
                    if create.snapshot.as_ref() == Some(snapshot) && !create.observation_valid {
                        if let WorkerExecutionState::Creating(create) = &mut self.execution {
                            create.observation_valid = true;
                        }
                        return true;
                    }
                    return false;
                }
                self.retain_candidate_snapshot(snapshot.clone());
                return true;
            }
            return false;
        }
        let active = self.observation_execution().expect("publication owner");
        if snapshot.state_sequence < active.snapshot.state_sequence
            || (snapshot == &active.snapshot && active.observation_valid)
        {
            return false;
        }
        if active.snapshot.state_sequence != snapshot.state_sequence {
            self.restore_guard.rotate();
        }
        self.remember_finished_submission(snapshot);
        let active = self.observation_execution_mut().expect("publication owner");
        active.snapshot = snapshot.clone();
        active.observation_valid = true;
        true
    }

    fn restored_detail(&self) -> WorkerDetail {
        let mut candidate = self.clone();
        candidate.settle_restore();
        candidate.detail()
    }
}

impl WorkerRecord {
    fn require_recovery(&mut self) {
        let operation = match &self.execution {
            WorkerExecutionState::Creating(operation) => {
                WorkerLifecycleOperation::Create(operation.clone())
            }
            WorkerExecutionState::Restoring(operation) => {
                WorkerLifecycleOperation::Restore(operation.clone())
            }
            WorkerExecutionState::Stopping(operation) => {
                WorkerLifecycleOperation::Stop(operation.clone())
            }
            _ => return,
        };
        self.execution = WorkerExecutionState::RecoveryRequired(operation);
    }
}

impl WorkerRecord {
    fn restore_operation_id(&self) -> Option<WorkerLifecycleOperationId> {
        #[cfg(feature = "fs-store")]
        if let WorkerExecutionState::RecoveryRequired(
            WorkerLifecycleOperation::RestoreFinalization(finalization),
        ) = &self.execution
        {
            return Some(finalization.operation_id);
        }
        self.pending_restore().map(|pending| pending.operation_id)
    }

    fn attest_existing_execution(
        &mut self,
        snapshot: &protocol::WorkerStateSnapshot,
    ) -> Result<(), RuntimeError> {
        if self.active_execution().is_some() {
            self.apply_worker_state(snapshot);
            return Ok(());
        }
        let worker_id = self.worker_id;
        let owner_not_started = self
            .restore_guard
            .active_request_id
            .as_ref()
            .and_then(|id| self.restore_guard.owners.get(id))
            .is_some_and(|owner| !owner.execution_started);
        if !owner_not_started {
            return Err(RuntimeError::RestoreObservationConflict { worker_id });
        }
        let restore = self
            .restore_operation_mut()
            .ok_or(RuntimeError::RestoreObservationConflict { worker_id })?;
        if restore.pending.source_repair_possible {
            return Err(RuntimeError::RestoreObservationConflict { worker_id });
        }
        let children = restore
            .candidate
            .execution()
            .map(|active| active.internal_workers.clone())
            .unwrap_or_default();
        restore.candidate = RestoreCandidate::Publishing(ActiveWorkerExecution {
            snapshot: snapshot.clone(),
            owner: restore
                .candidate
                .execution()
                .and_then(|active| active.owner),
            observation_valid: true,
            internal_workers: children,
        });
        // No new execution was allocated: the backend attested the existing one.
        // Its accepted no-op decision is saved atomically with the public receipt.
        restore.pending.commit = RestoreCommitFact::Committed;
        self.remember_finished_submission(snapshot);
        Ok(())
    }
}

impl WorkerRecord {
    fn observation_activity_mut(&mut self) -> Option<&mut InternalWorkerActivityProjection> {
        if matches!(
            self.execution,
            WorkerExecutionState::Creating(_)
                | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Create(_))
        ) {
            return match &mut self.execution {
                WorkerExecutionState::Creating(create)
                | WorkerExecutionState::RecoveryRequired(WorkerLifecycleOperation::Create(
                    create,
                )) => Some(&mut create.internal_workers),
                _ => None,
            };
        }
        self.observation_execution_mut()
            .map(|active| &mut active.internal_workers)
    }
}
