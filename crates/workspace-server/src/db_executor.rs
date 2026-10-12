//! Bounded execution of whole synchronous DB operations, independent of Tokio.
//!
//! Accepted jobs belong to the workers, not their async waiters. Cancelling a
//! waiter does not cancel or retry SQL, release capacity, or release its lease.
use std::{
    collections::BTreeSet,
    future::Future,
    num::NonZeroUsize,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex, OnceLock, mpsc},
};
use tokio::sync::{Notify, oneshot};

use crate::{Error, Result};

/// The default is one FIFO worker and at most 32 waiting operations.
#[derive(Clone, Copy, Debug)]
pub struct DbExecutorLimits {
    pub workers: NonZeroUsize,
    pub queued: NonZeroUsize,
}

impl Default for DbExecutorLimits {
    fn default() -> Self {
        Self {
            workers: NonZeroUsize::new(1).unwrap(),
            queued: NonZeroUsize::new(32).unwrap(),
        }
    }
}

/// A request can retain its lifecycle authority until actual SQL finishes.
/// Retention happens at submission, while the caller's task-local scope exists.
pub(crate) trait DbOperationLifetime: Send + Sync {
    fn retain(&self) -> Box<dyn Send>;
}

tokio::task_local! {
    static OPERATION_LIFETIME: Arc<dyn DbOperationLifetime>;
}

pub(crate) async fn with_operation_lifetime<F: Future>(
    lifetime: Arc<dyn DbOperationLifetime>,
    future: F,
) -> F::Output {
    OPERATION_LIFETIME.scope(lifetime, future).await
}

#[derive(Default)]
struct Accepted {
    last_job_id: u64,
    unfinished: BTreeSet<u64>,
}

#[derive(Default)]
struct Tracking {
    accepted: Mutex<Accepted>,
    finished: Notify,
}

struct Completion {
    tracking: Arc<Tracking>,
    id: u64,
}

impl Drop for Completion {
    fn drop(&mut self) {
        self.tracking
            .accepted
            .lock()
            .unwrap()
            .unfinished
            .remove(&self.id);
        self.tracking.finished.notify_waiters();
    }
}

struct Job {
    operation: Box<dyn FnOnce() + Send>,
    // Field drop order releases authority before completion becomes observable.
    _lifetime: Option<Box<dyn Send>>,
    lifetime_source: Option<Arc<dyn DbOperationLifetime>>,
    _completion: Completion,
    runtime: Option<tokio::runtime::Handle>,
    diagnostics: tracing::Dispatch,
    span: tracing::Span,
}

fn report_unknown(accepted_job_id: u64, phase: &'static str) {
    tracing::error!(
        code = "db_operation_outcome_unknown",
        accepted_job_id,
        phase,
        error = %Error::DbOperationOutcomeUnknown,
        "DB job panicked; outcome unknown, operation will not be retried"
    );
}

/// Clones share the queue and actual-job budget. Workers retain only the
/// receiver, so dropping the last sender closes the queue; accepted jobs finish
/// before the dedicated threads exit. No thread is created per store open.
#[derive(Clone)]
pub struct DbExecutor {
    sender: Option<mpsc::SyncSender<Job>>,
    tracking: Arc<Tracking>,
    max_jobs: usize,
}

impl DbExecutor {
    pub fn new(limits: DbExecutorLimits) -> Result<Self> {
        let max_jobs = limits
            .workers
            .get()
            .checked_add(limits.queued.get())
            .ok_or_else(|| Error::InvalidInput("DB executor limits overflow".into()))?;
        let (sender, receiver) = mpsc::sync_channel::<Job>(limits.queued.get());
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..limits.workers.get() {
            let receiver = receiver.clone();
            std::thread::Builder::new()
                .name(format!("workspace-db-{index}"))
                .spawn(move || {
                    loop {
                        let next = receiver.lock().unwrap().recv();
                        let Ok(job) = next else { break };
                        let Job {
                            operation,
                            _lifetime: lifetime,
                            lifetime_source,
                            _completion: completion,
                            runtime,
                            diagnostics,
                            span,
                        } = job;
                        // Keep diagnostics with their submitting subscriber as
                        // well as the runtime; no process-global override.
                        let _diagnostics = tracing::dispatcher::set_default(&diagnostics);
                        let _span = span.enter();
                        // Preserve the submitting runtime for synchronous Host
                        // post-commit hooks, including reply/lease destructors.
                        // enter() supplies context only: never drive a future
                        // or await/nest an executor submission in a transaction.
                        let _entered = runtime.as_ref().map(tokio::runtime::Handle::enter);
                        // A cancelled waiter's discarded result may itself have
                        // a destructor. Keep both the lease and actual-job slot
                        // until that cleanup finishes, even if it panics.
                        let execute = || {
                            if catch_unwind(AssertUnwindSafe(operation)).is_err() {
                                report_unknown(completion.id, "reply_cleanup");
                            }
                            if catch_unwind(AssertUnwindSafe(|| drop(lifetime))).is_err() {
                                report_unknown(completion.id, "lifetime_cleanup");
                            }
                        };
                        // Synchronous post-commit continuations can admit another
                        // bounded job. Transfer the original lifecycle source as
                        // well, so its authority survives the parent job/waiter.
                        // This does not inject context into detached Tokio tasks.
                        if let Some(source) = lifetime_source {
                            OPERATION_LIFETIME.sync_scope(source, execute);
                        } else {
                            execute();
                        }
                        drop(completion);
                    }
                })
                .map_err(|_| Error::DbExecutorUnavailable)?;
        }
        Ok(Self {
            sender: Some(sender),
            tracking: Arc::default(),
            max_jobs,
        })
    }

    pub fn shared() -> Self {
        static SHARED: OnceLock<DbExecutor> = OnceLock::new();
        SHARED
            .get_or_init(|| {
                Self::new(DbExecutorLimits::default()).unwrap_or_else(|_| Self {
                    sender: None,
                    tracking: Arc::default(),
                    max_jobs: 0,
                })
            })
            .clone()
    }

    /// Submit without waiting for queue capacity. The closure must contain the
    /// entire synchronous operation/transaction, with no nested async runtime.
    /// The submitting Tokio handle, when present, is entered for synchronous
    /// post-commit hooks and worker cleanup; it is never used to block_on SQL.
    pub async fn run<T, E>(
        &self,
        operation: impl FnOnce() -> std::result::Result<T, E> + Send + 'static,
    ) -> std::result::Result<T, E>
    where
        T: Send + 'static,
        E: From<Error> + Send + 'static,
    {
        self.submit(operation)?
            .await
            .unwrap_or_else(|_| Err(Error::DbOperationOutcomeUnknown.into()))
    }

    /// Admit a fire-and-forget operation immediately, without spawning a Tokio
    /// task or waiting for capacity. Err means the operation was not accepted;
    /// Ok means the worker owns it through execution and cleanup, even after
    /// the submitting task is cancelled. A panic emits a structured unknown-
    /// outcome diagnostic, never an automatic retry. drain() tracks these jobs.
    pub fn dispatch(&self, operation: impl FnOnce() + Send + 'static) -> Result<()> {
        self.submit(move || {
            operation();
            Ok::<_, Error>(())
        })
        .map(drop)
    }

    /// Shared synchronous admission: neither dropped receivers nor cancelled
    /// callers remove accepted work from the queue or the actual-job budget.
    fn submit<T, E>(
        &self,
        operation: impl FnOnce() -> std::result::Result<T, E> + Send + 'static,
    ) -> std::result::Result<oneshot::Receiver<std::result::Result<T, E>>, E>
    where
        T: Send + 'static,
        E: From<Error> + Send + 'static,
    {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| E::from(Error::DbExecutorUnavailable))?;
        let runtime = tokio::runtime::Handle::try_current().ok();
        let diagnostics = tracing::dispatcher::get_default(Clone::clone);
        let span = tracing::Span::current();
        let lifetime_source = OPERATION_LIFETIME.try_with(Clone::clone).ok();
        let lifetime = lifetime_source.as_ref().map(|source| source.retain());
        let (reply, result) = oneshot::channel();
        // Serialize admission and acceptance snapshots. Worker completion cannot
        // race acceptance, and cancelled waiters never alter this actual-job cap.
        {
            let mut accepted = self.tracking.accepted.lock().unwrap();
            if accepted.unfinished.len() >= self.max_jobs {
                return Err(Error::DbCapacityExceeded.into());
            }
            let id = accepted
                .last_job_id
                .checked_add(1)
                .ok_or_else(|| E::from(Error::DbExecutorUnavailable))?;
            // Never reuse the identity of a rejected job: its destructor may run
            // after another submission has acquired the admission lock.
            accepted.last_job_id = id;
            let job = Job {
                operation: Box::new(move || {
                    let outcome = catch_unwind(AssertUnwindSafe(operation)).unwrap_or_else(|_| {
                        report_unknown(id, "operation");
                        Err(Error::DbOperationOutcomeUnknown.into())
                    });
                    let _ = reply.send(outcome);
                }),
                _lifetime: lifetime,
                lifetime_source,
                runtime,
                diagnostics,
                span,
                _completion: Completion {
                    tracking: self.tracking.clone(),
                    id,
                },
            };
            match sender.try_send(job) {
                Ok(()) => {
                    accepted.unfinished.insert(id);
                }
                Err(error) => {
                    // The rejected Job's completion also locks tracking; drop it only
                    // after releasing the admission lock.
                    drop(accepted);
                    return Err(match error {
                        mpsc::TrySendError::Full(_) => Error::DbCapacityExceeded,
                        mpsc::TrySendError::Disconnected(_) => Error::DbExecutorUnavailable,
                    }
                    .into());
                }
            }
        }
        Ok(result)
    }

    /// Wait for every job accepted before this barrier is first polled (not
    /// later submissions). This bypasses queue capacity and survives cancelled
    /// job waiters. It does not close the queue or stop new submissions.
    pub async fn drain(&self) -> Result<()> {
        if self.sender.is_none() {
            return Err(Error::DbExecutorUnavailable);
        }
        let through = self.tracking.accepted.lock().unwrap().last_job_id;
        loop {
            let notified = self.tracking.finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .tracking
                .accepted
                .lock()
                .unwrap()
                .unfinished
                .first()
                .is_none_or(|id| *id > through)
            {
                return Ok(());
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, TransactionBehavior, ffi};
    use std::{ffi::c_void, time::Duration};

    fn executor(queued: usize) -> DbExecutor {
        DbExecutor::new(DbExecutorLimits {
            workers: NonZeroUsize::new(1).unwrap(),
            queued: NonZeroUsize::new(queued).unwrap(),
        })
        .unwrap()
    }

    fn database() -> (tempfile::TempDir, Connection, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.sqlite");
        let owner = Connection::open(&path).unwrap();
        owner
            .execute_batch("CREATE TABLE writes (value INTEGER NOT NULL)")
            .unwrap();
        let worker = Connection::open(&path).unwrap();
        (dir, owner, worker)
    }

    struct BusyGate {
        waiting: Option<oneshot::Sender<()>>,
        release: mpsc::Receiver<()>,
    }

    // A fixture-owned callback proves SQLite actually reached its lock wait.
    // Unlike busy_handler's fn pointer API, the SQLite API carries a per-call
    // context, so tests need neither process-global state nor guessed sleeps.
    struct BusyRegistration {
        db: *mut ffi::sqlite3,
        _gate: Box<BusyGate>,
    }

    impl BusyRegistration {
        fn install(conn: &Connection, gate: BusyGate) -> Self {
            unsafe extern "C" fn wait(context: *mut c_void, _: i32) -> i32 {
                // The Box remains registered only while this connection and
                // BusyRegistration are alive, on this same dedicated worker.
                let gate = unsafe { &mut *context.cast::<BusyGate>() };
                if let Some(waiting) = gate.waiting.take() {
                    let _ = waiting.send(());
                    if gate.release.recv().is_ok() {
                        return 1;
                    }
                }
                0
            }
            let mut gate = Box::new(gate);
            // SAFETY: conn owns the handle; registration is dropped before conn.
            let db = unsafe { conn.handle() };
            let result = unsafe {
                ffi::sqlite3_busy_handler(db, Some(wait), (&mut *gate as *mut BusyGate).cast())
            };
            assert_eq!(result, ffi::SQLITE_OK);
            Self { db, _gate: gate }
        }
    }

    impl Drop for BusyRegistration {
        fn drop(&mut self) {
            // SAFETY: unregister before releasing the callback's boxed context.
            unsafe {
                ffi::sqlite3_busy_handler(self.db, None, std::ptr::null_mut());
            }
        }
    }

    async fn observed<T>(future: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), future)
            .await
            .expect("DB job did not reach the expected observable state")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sqlite_lock_wait_preserves_timer_and_independent_control_progress() {
        let (_dir, mut owner, mut worker) = database();
        let lock = owner
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let executor = executor(1);
        let (waiting, blocked) = oneshot::channel();
        let (release, permit) = mpsc::channel();
        let mut write = Box::pin(executor.run(move || -> Result<()> {
            let _busy = BusyRegistration::install(
                &worker,
                BusyGate {
                    waiting: Some(waiting),
                    release: permit,
                },
            );
            let tx = worker.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("INSERT INTO writes VALUES (1)", [])?;
            tx.commit()?;
            Ok(())
        }));
        assert!(futures::poll!(&mut write).is_pending());
        observed(blocked).await.unwrap();
        let control = tokio::spawn(async { 42 });
        assert_eq!(observed(control).await.unwrap(), 42);
        // The timer itself is the contract under test, not synchronization.
        observed(tokio::time::sleep(Duration::from_millis(1))).await;
        assert!(futures::poll!(&mut write).is_pending());
        lock.rollback().unwrap();
        release.send(()).unwrap();
        observed(write).await.unwrap();
        executor.drain().await.unwrap();
        assert_eq!(
            owner
                .query_row("SELECT count(*) FROM writes", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    struct Lease(Arc<std::sync::atomic::AtomicUsize>);
    struct Retained(Arc<std::sync::atomic::AtomicUsize>);
    impl DbOperationLifetime for Lease {
        fn retain(&self) -> Box<dyn Send> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::new(Retained(self.0.clone()))
        }
    }
    impl Drop for Retained {
        fn drop(&mut self) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_queued_mutation_retains_capacity_and_lease_until_sql_commits() {
        let (_dir, mut owner, mut worker) = database();
        let lock = owner
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let executor = executor(1);
        let (waiting, blocked) = oneshot::channel();
        let (release, permit) = mpsc::channel();
        let mut running = Box::pin(executor.run(move || -> Result<()> {
            let _busy = BusyRegistration::install(
                &worker,
                BusyGate {
                    waiting: Some(waiting),
                    release: permit,
                },
            );
            let tx = worker.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("INSERT INTO writes VALUES (1)", [])?;
            tx.commit()?;
            Ok(())
        }));
        assert!(futures::poll!(&mut running).is_pending());
        observed(blocked).await.unwrap();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let lease: Arc<dyn DbOperationLifetime> = Arc::new(Lease(count.clone()));
        let mut queued_conn = Connection::open(_dir.path().join("jobs.sqlite")).unwrap();
        let mut queued = Box::pin(with_operation_lifetime(
            lease,
            executor.run(move || -> Result<()> {
                let tx = queued_conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                // FIFO is observable in durable SQL, not inferred from waiter order.
                let first: i64 =
                    tx.query_row("SELECT count(*) FROM writes WHERE value = 1", [], |row| {
                        row.get(0)
                    })?;
                assert_eq!(first, 1);
                tx.execute("INSERT INTO writes VALUES (2)", [])?;
                tx.commit()?;
                Ok(())
            }),
        ));
        assert!(futures::poll!(&mut queued).is_pending());
        drop(queued);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(matches!(
            executor.run(|| Ok::<_, Error>(())).await,
            Err(Error::DbCapacityExceeded)
        ));
        let mut drained = Box::pin(executor.drain());
        assert!(futures::poll!(&mut drained).is_pending());
        lock.rollback().unwrap();
        release.send(()).unwrap();
        observed(running).await.unwrap();
        observed(drained).await.unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            owner
                .query_row("SELECT count(*) FROM writes", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        executor.run(|| Ok::<_, Error>(())).await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn post_commit_child_job_retains_admitted_lifetime_after_parent_waiter_and_job_finish() {
        let (dir, owner, mut parent_conn) = database();
        let mut child_conn = Connection::open(dir.path().join("jobs.sqlite")).unwrap();
        let parent = executor(1);
        let child = executor(1);
        let child_owner = child.clone();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let source: Arc<dyn DbOperationLifetime> = Arc::new(Lease(count.clone()));
        let (entered, running) = oneshot::channel();
        let (release, permit) = mpsc::channel();
        let (child_entered, child_running) = oneshot::channel();
        let (finish_child, child_permit) = mpsc::channel();
        let mut operation = Box::pin(with_operation_lifetime(
            source,
            parent.run(move || -> Result<()> {
                let _ = entered.send(());
                let _ = permit.recv();
                let tx = parent_conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute("INSERT INTO writes VALUES (1)", [])?;
                tx.commit()?;
                child_owner.dispatch(move || {
                    let tx = child_conn
                        .transaction_with_behavior(TransactionBehavior::Immediate)
                        .unwrap();
                    let committed: i64 = tx
                        .query_row("SELECT count(*) FROM writes", [], |row| row.get(0))
                        .unwrap();
                    assert_eq!(committed, 1, "child must see its parent's committed SQL");
                    let _ = child_entered.send(());
                    let _ = child_permit.recv();
                    tx.execute("INSERT INTO writes VALUES (2)", []).unwrap();
                    tx.commit().unwrap();
                })?;
                Ok(())
            }),
        ));
        assert!(futures::poll!(&mut operation).is_pending());
        observed(running).await.unwrap();
        drop(operation);
        release.send(()).unwrap();
        observed(child_running).await.unwrap();
        observed(parent.drain()).await.unwrap();
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "accepted continuation lost authority when its parent finished"
        );
        let mut drained = Box::pin(child.drain());
        assert!(futures::poll!(&mut drained).is_pending());
        finish_child.send(()).unwrap();
        observed(drained).await.unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            owner
                .query_row("SELECT count(*) FROM writes", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn failed_transaction_rolls_back_and_releases_actual_job_capacity() {
        let (_dir, owner, mut worker) = database();
        let executor = executor(1);
        let error = executor
            .run(move || -> Result<()> {
                let tx = worker.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute("INSERT INTO writes VALUES (1)", [])?;
                tx.execute("INSERT INTO writes VALUES (NULL)", [])?;
                tx.commit()?;
                Ok(())
            })
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Sqlite(_)));
        executor.drain().await.unwrap();
        assert_eq!(
            owner
                .query_row("SELECT count(*) FROM writes", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        executor.run(|| Ok::<_, Error>(())).await.unwrap();
    }

    #[tokio::test]
    async fn panic_after_commit_reports_unknown_without_retry_and_worker_remains_available() {
        let (_dir, owner, mut worker) = database();
        let executor = executor(1);
        let error = executor
            .run(move || -> Result<()> {
                let tx = worker.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute("INSERT INTO writes VALUES (1)", [])?;
                tx.commit()?;
                panic!("lost outcome after commit");
            })
            .await
            .unwrap_err();
        assert!(matches!(error, Error::DbOperationOutcomeUnknown));
        executor.drain().await.unwrap();
        assert_eq!(
            owner
                .query_row("SELECT count(*) FROM writes", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(executor.run(|| Ok::<_, Error>(42)).await.unwrap(), 42);
    }

    #[tokio::test]
    async fn panic_before_commit_rolls_back_and_releases_lease_and_capacity() {
        let (_dir, owner, mut worker) = database();
        let executor = executor(1);
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let lease: Arc<dyn DbOperationLifetime> = Arc::new(Lease(count.clone()));
        let error = with_operation_lifetime(
            lease,
            executor.run(move || -> Result<()> {
                let tx = worker.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute("INSERT INTO writes VALUES (1)", [])?;
                panic!("lost outcome before commit");
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, Error::DbOperationOutcomeUnknown));
        executor.drain().await.unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            owner
                .query_row("SELECT count(*) FROM writes", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        executor.run(|| Ok::<_, Error>(())).await.unwrap();
    }

    struct DiscardedReplyHook {
        expected_runtime: tokio::runtime::Id,
        lease_count: Arc<std::sync::atomic::AtomicUsize>,
        dropped: Option<oneshot::Sender<(bool, usize)>>,
    }

    impl Drop for DiscardedReplyHook {
        fn drop(&mut self) {
            let runtime =
                tokio::runtime::Handle::try_current().expect("reply cleanup lost runtime context");
            let retained = self.lease_count.load(std::sync::atomic::Ordering::SeqCst);
            let same_runtime = runtime.id() == self.expected_runtime;
            let dropped = self.dropped.take().unwrap();
            tokio::spawn(async move {
                let _ = dropped.send((same_runtime, retained));
            });
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_queued_sql_keeps_runtime_context_and_lease_through_post_commit_and_reply_drop()
     {
        let (_dir, owner, mut worker) = database();
        let executor = executor(1);
        let (started, running) = oneshot::channel();
        let (release, permit) = mpsc::channel();
        let mut blocker = Box::pin(executor.run(move || -> Result<()> {
            let _ = started.send(());
            let _ = permit.recv();
            Ok(())
        }));
        assert!(futures::poll!(&mut blocker).is_pending());
        observed(running).await.unwrap();
        let expected_runtime = tokio::runtime::Handle::current().id();
        let (hook, published) = oneshot::channel();
        let (finish, finishing) = mpsc::channel();
        let (dropped, reply_dropped) = oneshot::channel();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let lease: Arc<dyn DbOperationLifetime> = Arc::new(Lease(count.clone()));
        let operation_count = count.clone();
        let mut queued = Box::pin(with_operation_lifetime(
            lease,
            executor.run(move || -> Result<_> {
                let tx = worker.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute("INSERT INTO writes VALUES (1)", [])?;
                tx.commit()?;
                // Model synchronous Host hooks: publish only after commit, checking
                // try_current and spawning without driving any future on this worker.
                let runtime = tokio::runtime::Handle::try_current()
                    .expect("post-commit hook lost runtime context");
                assert_eq!(runtime.id(), expected_runtime);
                tokio::spawn(async move {
                    let _ = hook.send(());
                });
                let _ = finishing.recv();
                Ok(DiscardedReplyHook {
                    expected_runtime,
                    lease_count: operation_count,
                    dropped: Some(dropped),
                })
            }),
        ));
        assert!(futures::poll!(&mut queued).is_pending());
        drop(queued);
        release.send(()).unwrap();
        observed(blocker).await.unwrap();
        observed(published).await.unwrap();
        assert_eq!(
            owner
                .query_row("SELECT count(*) FROM writes", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        let mut drain = Box::pin(executor.drain());
        assert!(futures::poll!(&mut drain).is_pending());
        finish.send(()).unwrap();
        assert_eq!(observed(reply_dropped).await.unwrap(), (true, 1));
        observed(drain).await.unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
        executor.run(|| Ok::<_, Error>(())).await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn multiple_workers_bound_actual_jobs_even_after_all_waiters_are_cancelled() {
        let executor = DbExecutor::new(DbExecutorLimits {
            workers: NonZeroUsize::new(2).unwrap(),
            queued: NonZeroUsize::new(1).unwrap(),
        })
        .unwrap();
        let mut releases = Vec::new();
        // Observe each worker executing before submitting the next operation,
        // so queue occupancy and overload are deterministic, not timing guesses.
        for _ in 0..2 {
            let (started, running) = oneshot::channel();
            let (release, permit) = mpsc::channel();
            let mut operation = Box::pin(executor.run(move || -> Result<()> {
                let _ = started.send(());
                let _ = permit.recv();
                Ok(())
            }));
            assert!(futures::poll!(&mut operation).is_pending());
            observed(running).await.unwrap();
            drop(operation);
            releases.push(release);
        }
        let (finished, mut queued_finished) = oneshot::channel();
        let mut queued = Box::pin(executor.run(move || -> Result<()> {
            let _ = finished.send(());
            Ok(())
        }));
        assert!(futures::poll!(&mut queued).is_pending());
        drop(queued);
        let rejected_runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for _ in 0..8 {
            let rejected_runs = rejected_runs.clone();
            assert!(matches!(
                executor
                    .run(move || {
                        rejected_runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok::<_, Error>(())
                    })
                    .await,
                Err(Error::DbCapacityExceeded)
            ));
        }
        assert_eq!(rejected_runs.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(matches!(
            queued_finished.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        let mut drain = Box::pin(executor.drain());
        assert!(futures::poll!(&mut drain).is_pending());
        for release in releases {
            release.send(()).unwrap();
        }
        observed(drain).await.unwrap();
        observed(queued_finished).await.unwrap();
        executor.run(|| Ok::<_, Error>(())).await.unwrap();
    }

    #[derive(Default)]
    struct UnknownDiagnostic {
        code: Option<String>,
        phase: Option<String>,
        accepted_job_id: Option<u64>,
    }

    impl tracing::field::Visit for UnknownDiagnostic {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            match field.name() {
                "code" => self.code = Some(value.to_owned()),
                "phase" => self.phase = Some(value.to_owned()),
                _ => {}
            }
        }
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            if field.name() == "accepted_job_id" {
                self.accepted_job_id = Some(value);
            }
        }
        fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
    }

    #[derive(Clone, Default)]
    struct DiagnosticCapture(Arc<Mutex<Vec<UnknownDiagnostic>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for DiagnosticCapture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut diagnostic = UnknownDiagnostic::default();
            event.record(&mut diagnostic);
            self.0.lock().unwrap().push(diagnostic);
        }
    }

    struct DispatchCleanup {
        expected_runtime: tokio::runtime::Id,
        lease_count: Arc<std::sync::atomic::AtomicUsize>,
        started: Option<oneshot::Sender<(bool, usize)>>,
        release: mpsc::Receiver<()>,
    }

    impl Drop for DispatchCleanup {
        fn drop(&mut self) {
            // No panicking assertions during unwind: report observations to the
            // test, and gate cleanup so drain/lifetime behavior is observable.
            let same_runtime = tokio::runtime::Handle::try_current()
                .is_ok_and(|runtime| runtime.id() == self.expected_runtime);
            let retained = self.lease_count.load(std::sync::atomic::Ordering::SeqCst);
            if let Some(started) = self.started.take() {
                let _ = started.send((same_runtime, retained));
            }
            let _ = self.release.recv();
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatched_panic_is_bounded_retained_through_cleanup_diagnosed_and_drained_without_retry()
     {
        use tracing_subscriber::layer::SubscriberExt;
        let (_dir, owner, mut worker) = database();
        let executor = executor(1);
        let (started, running) = oneshot::channel();
        let (release, permit) = mpsc::channel();
        let mut blocker = Box::pin(executor.run(move || -> Result<()> {
            let _ = started.send(());
            let _ = permit.recv();
            Ok(())
        }));
        assert!(futures::poll!(&mut blocker).is_pending());
        observed(running).await.unwrap();
        drop(blocker); // Cancellation does not free its running slot.
        let expected_runtime = tokio::runtime::Handle::current().id();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let lease: Arc<dyn DbOperationLifetime> = Arc::new(Lease(count.clone()));
        let operation_count = count.clone();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let operation_attempts = attempts.clone();
        let (cleanup_started, cleaning_up) = oneshot::channel();
        let (finish, finishing) = mpsc::channel();
        // Model multiple tracing contexts without setting a global subscriber.
        // This also prevents tracing's single-subscriber callsite optimization
        // from caching an unsubscribed parallel test's Interest::never here.
        let _other_diagnostics = tracing::Dispatch::new(tracing_subscriber::registry());
        let captured = DiagnosticCapture::default();
        let diagnostics =
            tracing::Dispatch::new(tracing_subscriber::registry().with(captured.clone()));
        with_operation_lifetime(lease, async {
            tracing::dispatcher::with_default(&diagnostics, || {
                executor.dispatch(move || {
                    let _cleanup = DispatchCleanup {
                        expected_runtime,
                        lease_count: operation_count,
                        started: Some(cleanup_started),
                        release: finishing,
                    };
                    operation_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let tx = worker
                        .transaction_with_behavior(TransactionBehavior::Immediate)
                        .unwrap();
                    tx.execute("INSERT INTO writes VALUES (1)", []).unwrap();
                    tx.commit().unwrap();
                    panic!("dispatched post-commit outcome lost");
                })
            })
        })
        .await
        .unwrap(); // Dispatch is admitted synchronously; no waiter survives.
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        let rejected_runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rejected_operation = rejected_runs.clone();
        assert!(matches!(
            executor.dispatch(move || {
                rejected_operation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
            Err(Error::DbCapacityExceeded)
        ));
        release.send(()).unwrap();
        assert_eq!(observed(cleaning_up).await.unwrap(), (true, 1));
        assert_eq!(
            owner
                .query_row("SELECT count(*) FROM writes", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let mut drain = Box::pin(executor.drain());
        assert!(futures::poll!(&mut drain).is_pending());
        finish.send(()).unwrap();
        observed(drain).await.unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(rejected_runs.load(std::sync::atomic::Ordering::SeqCst), 0);
        {
            let events = captured.0.lock().unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(
                events[0].code.as_deref(),
                Some("db_operation_outcome_unknown")
            );
            assert_eq!(events[0].phase.as_deref(), Some("operation"));
            assert!(events[0].accepted_job_id.is_some());
        }
        let (completed, completion) = oneshot::channel();
        executor
            .dispatch(move || {
                let _ = completed.send(());
            })
            .unwrap();
        observed(executor.drain()).await.unwrap();
        observed(completion).await.unwrap();
    }
}
