//! Runtime-owned continuation of an admitted restore, independent of its caller.
//!
//! Completion callbacks only signal a condition variable. They never acquire a
//! Runtime/lifecycle lock and are never joined by this driver while holding one.
//! Each step revalidates the canonical owner under the existing Worker lock;
//! waiting/backoff and driver teardown happen outside it. A repeated obstruction
//! parks this owner with its reason, rather than spinning or inventing rollback.

use super::*;
use crate::execution::RestoreRecoveryDisposition;
use std::collections::HashMap;
use std::sync::{Condvar, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

type Key = (WorkerRef, WorkerLifecycleOperationId);

#[derive(Debug, Default)]
pub(super) struct RestoreContinuations {
    owners: Mutex<HashMap<Key, Arc<Continuation>>>,
    #[cfg(test)]
    paused: std::sync::atomic::AtomicBool,
}

#[derive(Debug, Default)]
struct Schedule {
    signaled: bool,
    cancelled: bool,
    finished: bool,
    blocked_reason: Option<String>,
    parked: bool,
}

#[derive(Debug, Default)]
struct Continuation {
    schedule: Mutex<Schedule>,
    wake: Condvar,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl RestoreContinuations {
    pub(super) fn signal(&self, worker_ref: &WorkerRef, operation: WorkerLifecycleOperationId) {
        if let Some(owner) = self
            .owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(worker_ref.clone(), operation))
        {
            let mut schedule = owner
                .schedule
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            schedule.signaled = true;
            owner.wake.notify_all();
        }
    }
}

impl Drop for RestoreContinuations {
    fn drop(&mut self) {
        let owners = self
            .owners
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for owner in owners.values() {
            let mut schedule = owner
                .schedule
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            schedule.cancelled = true;
            owner.wake.notify_all();
        }
        for owner in owners.values() {
            if let Some(task) = owner
                .task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                if task.thread().id() != std::thread::current().id() {
                    let _ = task.join();
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) struct ContinuationPause(Arc<RestoreContinuations>);

#[cfg(test)]
impl Drop for ContinuationPause {
    fn drop(&mut self) {
        self.0
            .paused
            .store(false, std::sync::atomic::Ordering::Release);
        for owner in self.0.owners.lock().unwrap().values() {
            owner.schedule.lock().unwrap().signaled = true;
            owner.wake.notify_all();
        }
    }
}

enum Step {
    Finished,
    Waiting,
    Contended,
    Retry(String),
}

impl Runtime {
    pub(super) fn restore_repair_authorizer(
        &self,
        worker_ref: WorkerRef,
    ) -> Arc<dyn Fn(WorkerLifecycleOperationId) -> Result<(), RuntimeError> + Send + Sync> {
        let inner = Arc::downgrade(&self.inner);
        Arc::new(move |operation| {
            let inner = inner.upgrade().ok_or(RuntimeError::RuntimeStopped)?;
            // The factory caller owns the lifecycle lock. Never reacquire it
            // here: it may be awaiting this exact factory task.
            let mut state = inner.lock().map_err(|_| RuntimeError::StatePoisoned)?;
            state.ensure_running()?;
            state.refresh_uncertain_worker_write(&worker_ref)?;
            let worker = state.worker(&worker_ref)?;
            state.ensure_subject_not_reserved(&worker_ref, false)?;
            if worker.pending_stop.is_some()
                || worker.pending_restore.is_none_or(|pending| {
                    pending.operation_id != operation
                        || pending.commit == RestoreCommitFact::LegacyUnknown
                })
            {
                return Err(RuntimeError::RestoreObservationConflict {
                    worker_id: worker_ref.worker_id,
                });
            }
            if let Some(owner) = worker
                .restore_guard
                .active_request_id
                .as_ref()
                .and_then(|id| worker.restore_guard.owners.get(id))
            {
                owner.validate_workdir_preparation_authority(worker)?;
            }
            if worker
                .pending_restore
                .is_some_and(|pending| pending.source_repair_possible)
            {
                return Ok(());
            }
            let mut candidate = worker.clone();
            candidate
                .pending_restore
                .as_mut()
                .expect("validated restore owner")
                .source_repair_possible = true;
            // Grant before touching another canonical store. Unknown durability
            // denies repair until authoritative reconciliation; no old-memory write.
            state.persist_restore_worker_record(&candidate)?;
            state.workers.insert(worker_ref.worker_id, candidate);
            Ok(())
        })
    }

    #[cfg(test)]
    pub(crate) fn test_wait_restore_continuation_parked(
        &self,
        worker_ref: &WorkerRef,
        operation: WorkerLifecycleOperationId,
        timeout: Duration,
    ) -> String {
        let owner = self
            .restore_continuations
            .owners
            .lock()
            .unwrap()
            .get(&(worker_ref.clone(), operation))
            .cloned()
            .unwrap();
        let (schedule, waited) = owner
            .wake
            .wait_timeout_while(owner.schedule.lock().unwrap(), timeout, |s| {
                !s.parked && !s.finished
            })
            .unwrap();
        assert!(
            !waited.timed_out() && schedule.parked,
            "continuation did not park: {:?}",
            schedule.blocked_reason
        );
        schedule.blocked_reason.clone().expect("parked reason")
    }

    /// Gate autonomous steps when a component test observes a single fault
    /// boundary. Dedicated continuation tests run without this gate.
    #[cfg(test)]
    pub(crate) fn test_pause_restore_continuation(&self) -> ContinuationPause {
        assert!(
            !self
                .restore_continuations
                .paused
                .swap(true, std::sync::atomic::Ordering::AcqRel)
        );
        ContinuationPause(self.restore_continuations.clone())
    }

    #[cfg(test)]
    pub(crate) fn test_wait_restore_continuation(
        &self,
        worker_ref: &WorkerRef,
        operation: WorkerLifecycleOperationId,
        timeout: Duration,
    ) {
        let owner = self
            .restore_continuations
            .owners
            .lock()
            .unwrap()
            .get(&(worker_ref.clone(), operation))
            .cloned()
            .expect("continuation owner");
        let schedule = owner.schedule.lock().unwrap();
        let (schedule, waited) = owner
            .wake
            .wait_timeout_while(schedule, timeout, |s| !s.finished && !s.cancelled)
            .unwrap();
        assert!(
            !waited.timed_out() && schedule.finished,
            "restore continuation did not settle: {:?}",
            schedule.blocked_reason
        );
    }

    pub(super) fn ensure_restore_continuation(
        &self,
        worker_ref: &WorkerRef,
    ) -> Result<(), RuntimeError> {
        let operation = {
            let state = self.lock()?;
            let worker = state.worker(worker_ref)?;
            worker
                .pending_restore
                .map(|pending| pending.operation_id)
                .or_else(|| {
                    worker
                        .restore_guard
                        .active_request_id
                        .as_ref()
                        .and_then(|id| worker.restore_guard.owners.get(id))
                        .filter(|owner| owner.receipt.is_none())
                        .map(|owner| owner.operation_id)
                })
        };
        if let Some(operation) = operation {
            self.start_restore_continuation(worker_ref.clone(), operation)?;
        }
        Ok(())
    }

    pub(super) fn start_restore_continuation(
        &self,
        worker_ref: WorkerRef,
        operation: WorkerLifecycleOperationId,
    ) -> Result<(), RuntimeError> {
        {
            let state = self.lock()?;
            let Some(backend) = &state.execution_backend else {
                return Ok(());
            };
            if !backend.supports_restore_continuation()
                || backend
                    .restore_recovery_disposition(operation, &worker_ref)
                    .is_none()
            {
                return Ok(());
            }
        }
        let key = (worker_ref.clone(), operation);
        let mut owners = self
            .restore_continuations
            .owners
            .lock()
            .map_err(|_| RuntimeError::StatePoisoned)?;
        if owners.contains_key(&key) {
            return Ok(());
        }
        let owner = Arc::new(Continuation::default());
        let retained = owner.clone();
        let state = Arc::downgrade(&self.inner);
        let operations = Arc::downgrade(&self.worker_operations);
        let continuations = Arc::downgrade(&self.restore_continuations);
        let task = std::thread::Builder::new()
            .name("restore-continuation".into())
            .spawn(move || {
                run(
                    state,
                    operations,
                    continuations,
                    retained,
                    worker_ref,
                    operation,
                );
            })
            .map_err(|error| {
                RuntimeError::InvalidRequest(format!(
                    "Restore continuation could not start: {error}"
                ))
            })?;
        *owner.task.lock().map_err(|_| RuntimeError::StatePoisoned)? = Some(task);
        owners.insert(key, owner);
        Ok(())
    }

    fn continue_restore_operation(
        &self,
        worker_ref: &WorkerRef,
        operation: WorkerLifecycleOperationId,
    ) -> Result<Step, RuntimeError> {
        #[cfg(test)]
        if self
            .restore_continuations
            .paused
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(Step::Contended);
        }
        let lease = self.worker_operation_lock(worker_ref.worker_id)?;
        // Another caller may be waiting for a backend task. Do not queue a join
        // of that caller/driver while retaining the lock it needs to complete.
        let _guard = match lease.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(Step::Contended),
            Err(_) => return Err(RuntimeError::StatePoisoned),
        };
        let stop_is_owned = {
            let mut state = self.lock()?;
            state.ensure_running()?;
            state.refresh_uncertain_worker_write(worker_ref)?;
            let worker = state.worker(worker_ref)?;
            worker.pending_stop.is_some()
                && worker
                    .pending_restore
                    .is_some_and(|pending| pending.operation_id == operation)
        };
        if stop_is_owned {
            // Stop supersedes publication, not ownership of late resources.
            // Retain this same continuation until its cleanup is confirmed.
            self.reconcile_pending_stop_under_lock(worker_ref)?;
            return Ok(Step::Finished);
        }
        let (backend, pending) = {
            let mut state = self.lock()?;
            state.ensure_running()?;
            state.refresh_uncertain_worker_write(worker_ref)?;
            let worker = state.worker(worker_ref)?;
            if worker.pending_stop.is_some() {
                return Ok(Step::Finished);
            }
            let owner = worker
                .restore_guard
                .active_request_id
                .as_ref()
                .and_then(|id| worker.restore_guard.owners.get(id));
            if owner.is_some_and(|owner| owner.operation_id != operation || owner.receipt.is_some())
            {
                return Ok(Step::Finished);
            }
            let Some(pending) = worker.pending_restore else {
                return Ok(Step::Finished);
            };
            if pending.operation_id != operation {
                return Ok(Step::Finished);
            }
            if pending.commit == RestoreCommitFact::LegacyUnknown {
                return Ok(Step::Retry(
                    "Saved Restore has no authoritative commit evidence".into(),
                ));
            }
            state.ensure_subject_not_reserved(worker_ref, false)?;
            if let Some(owner) = owner {
                owner.validate_workdir_preparation_authority(worker)?;
            }
            let Some(backend) = state.execution_backend.clone() else {
                return Ok(Step::Finished);
            };
            (backend, pending)
        };
        match backend.restore_recovery_disposition(operation, worker_ref) {
            None => return Ok(Step::Finished),
            Some(RestoreRecoveryDisposition::InFlight) => return Ok(Step::Waiting),
            Some(RestoreRecoveryDisposition::Discard)
                if pending.commit == RestoreCommitFact::NotCommitted =>
            {
                match backend.discard_uncommitted_restore(operation, worker_ref) {
                    WorkerExecutionSpawnResult::RolledBack(result) => {
                        let receipt = RuntimeWorkerRestoreResult::failed(
                            WorkerRestoreState::RolledBack,
                            "worker_restore_rolled_back",
                            result.message_or_default(),
                        );
                        #[cfg(feature = "fs-store")]
                        self.lock()?
                            .record_restore_failure(worker_ref, &result, &receipt)?;
                        #[cfg(not(feature = "fs-store"))]
                        self.settle_restore_rollback(worker_ref, &receipt)?;
                        return Ok(Step::Finished);
                    }
                    result => {
                        return Ok(Step::Retry(format!(
                            "Restore cleanup remains unresolved: {result:?}"
                        )));
                    }
                }
            }
            Some(_) => {}
        }
        let result = self.restore_admitted_worker_under_lock(worker_ref, pending.mode)?;
        if backend.restore_recovery_disposition(operation, worker_ref)
            == Some(RestoreRecoveryDisposition::InFlight)
        {
            return Ok(Step::Waiting);
        }
        if result.state == WorkerRestoreState::ReconciliationRequired {
            Ok(Step::Retry(result.message.unwrap_or_else(|| {
                "Restore reconciliation remains pending".into()
            })))
        } else {
            Ok(Step::Finished)
        }
    }
}

fn run(
    inner: Weak<Mutex<RuntimeState>>,
    operations: Weak<Mutex<BTreeMap<WorkerId, Arc<Mutex<()>>>>>,
    continuations: Weak<RestoreContinuations>,
    owner: Arc<Continuation>,
    worker_ref: WorkerRef,
    operation: WorkerLifecycleOperationId,
) {
    let mut repeated = 0u32;
    let mut last_reason = None;
    loop {
        let (Some(inner), Some(worker_operations), Some(restore_continuations)) = (
            inner.upgrade(),
            operations.upgrade(),
            continuations.upgrade(),
        ) else {
            break;
        };
        owner
            .schedule
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .parked = false;
        let runtime = Runtime {
            inner,
            worker_operations,
            restore_continuations,
        };
        let step = runtime.continue_restore_operation(&worker_ref, operation);
        // No lifecycle lock or strong Runtime/registry reference survives waiting.
        drop(runtime);
        let mut schedule = owner
            .schedule
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if schedule.cancelled {
            break;
        }
        let contended = matches!(&step, Ok(Step::Contended));
        let reason = match step {
            Ok(Step::Finished) => {
                schedule.finished = true;
                owner.wake.notify_all();
                break;
            }
            Ok(Step::Waiting) | Ok(Step::Contended) => None,
            Ok(Step::Retry(reason)) => Some(reason),
            Err(error) => Some(sanitize_worker_create_failure_message(&error.to_string())),
        };
        if let Some(reason) = &reason {
            repeated = repeated.saturating_add(1);
            last_reason = Some(reason.clone());
            schedule.blocked_reason = Some(reason.clone());
        }
        // Completed failed attempts can have signaled while this step joined
        // them. Do not let our own unchanged failure reset the retry budget.
        if reason.is_some() {
            schedule.signaled = false;
        }
        if (!contended && reason.is_none()) || repeated >= 4 {
            if repeated >= 4 {
                schedule.parked = true;
                owner.wake.notify_all();
                tracing::warn!(worker_id = %worker_ref.worker_id, operation_id = %operation, reason = ?last_reason,
                    "Restore continuation parked after bounded attempts; operation remains unresolved");
            }
            schedule = owner
                .wake
                .wait_while(schedule, |s| !s.signaled && !s.cancelled)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        } else {
            let delay = Duration::from_millis(100 * (1u64 << repeated.min(4)));
            schedule = owner
                .wake
                .wait_timeout_while(schedule, delay, |s| !s.signaled && !s.cancelled)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        if schedule.cancelled {
            break;
        }
        if schedule.signaled {
            schedule.signaled = false;
            // A completion wakes this same owner, not a new request. Its next
            // observed result, not the signal itself, determines progress.
        }
    }
}
