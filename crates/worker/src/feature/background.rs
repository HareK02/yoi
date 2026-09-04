//! Executable, scope-owned background tasks contributed by Worker features.
//!
//! Tasks never receive provider handles, credentials, or raw Workdir paths from
//! this registry. Callers pass only stable Worker/session provenance. The task
//! implementation obtains any additional authority through the services its
//! feature was explicitly granted at install time.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use tokio::sync::Notify;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::{BackgroundTaskDeclaration, FeatureId, FeatureInstallError};
use crate::hook::{HookError, HookErrorCategory, HookInvocationContext};

const MAX_TASK_CONCURRENCY: u16 = 64;
const MAX_TASK_ATTEMPTS: u16 = 16;
const MAX_TASK_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_RETAINED_DIAGNOSTICS: usize = 128;
const TASK_SETTLE_TIMEOUT_MS: u64 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskRewritePolicy {
    CancelAndWait,
    Wait,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskShutdownPolicy {
    CancelAndWait,
    Wait,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskRetryPolicy {
    Never,
    Bounded { max_attempts: u16, delay_ms: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskTrigger {
    Manual,
    RunCommitted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundTaskSpec {
    pub declaration: BackgroundTaskDeclaration,
    pub trigger: BackgroundTaskTrigger,
    pub max_concurrency: u16,
    pub timeout_ms: u64,
    pub retry: BackgroundTaskRetryPolicy,
    pub rewrite: BackgroundTaskRewritePolicy,
    pub shutdown: BackgroundTaskShutdownPolicy,
}

impl BackgroundTaskSpec {
    pub fn single_flight(declaration: BackgroundTaskDeclaration, timeout: Duration) -> Self {
        Self {
            declaration,
            trigger: BackgroundTaskTrigger::Manual,
            max_concurrency: 1,
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            retry: BackgroundTaskRetryPolicy::Never,
            rewrite: BackgroundTaskRewritePolicy::CancelAndWait,
            shutdown: BackgroundTaskShutdownPolicy::CancelAndWait,
        }
    }

    fn validate(&self) -> Result<(), FeatureInstallError> {
        if self.max_concurrency == 0 || self.max_concurrency > MAX_TASK_CONCURRENCY {
            return Err(FeatureInstallError::InvalidDescriptor(format!(
                "background task `{}` max_concurrency must be within 1..={MAX_TASK_CONCURRENCY}",
                self.declaration.name
            )));
        }
        if self.timeout_ms == 0 || self.timeout_ms > MAX_TASK_TIMEOUT_MS {
            return Err(FeatureInstallError::InvalidDescriptor(format!(
                "background task `{}` timeout_ms must be within 1..={MAX_TASK_TIMEOUT_MS}",
                self.declaration.name
            )));
        }
        if let BackgroundTaskRetryPolicy::Bounded { max_attempts, .. } = self.retry
            && (max_attempts == 0 || max_attempts > MAX_TASK_ATTEMPTS)
        {
            return Err(FeatureInstallError::InvalidDescriptor(format!(
                "background task `{}` max_attempts must be within 1..={MAX_TASK_ATTEMPTS}",
                self.declaration.name
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundTaskContext {
    pub invocation: HookInvocationContext,
    pub feature_id: FeatureId,
    pub task_name: String,
    pub execution_id: u64,
    pub session_generation: u64,
    pub attempt: u16,
}

#[derive(Clone)]
pub struct BackgroundTaskCancellation {
    sender: Arc<watch::Sender<bool>>,
}

impl Default for BackgroundTaskCancellation {
    fn default() -> Self {
        let (sender, _receiver) = watch::channel(false);
        Self {
            sender: Arc::new(sender),
        }
    }
}

impl BackgroundTaskCancellation {
    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        if *receiver.borrow_and_update() {
            return;
        }
        while receiver.changed().await.is_ok() {
            if *receiver.borrow_and_update() {
                return;
            }
        }
    }

    fn cancel(&self) {
        self.sender.send_replace(true);
    }
}

#[async_trait]
pub trait FeatureBackgroundTask: Send + Sync {
    async fn run(
        &self,
        context: BackgroundTaskContext,
        cancellation: BackgroundTaskCancellation,
    ) -> Result<(), HookError>;
}

struct Registration {
    feature_id: FeatureId,
    spec: BackgroundTaskSpec,
    task: Arc<dyn FeatureBackgroundTask>,
}

#[derive(Default)]
pub struct FeatureBackgroundTaskRegistryBuilder {
    registrations: BTreeMap<(FeatureId, String), Registration>,
}

impl FeatureBackgroundTaskRegistryBuilder {
    pub fn register(
        &mut self,
        feature_id: FeatureId,
        spec: BackgroundTaskSpec,
        task: impl FeatureBackgroundTask + 'static,
    ) -> Result<(), FeatureInstallError> {
        spec.validate()?;
        let key = (feature_id.clone(), spec.declaration.name.clone());
        if self.registrations.contains_key(&key) {
            return Err(FeatureInstallError::InvalidDescriptor(format!(
                "feature `{feature_id}` registered background task `{}` more than once",
                spec.declaration.name
            )));
        }
        self.registrations.insert(
            key,
            Registration {
                feature_id,
                spec,
                task: Arc::new(task),
            },
        );
        Ok(())
    }

    pub(crate) fn checkpoint(&self) -> Vec<(FeatureId, String)> {
        self.registrations.keys().cloned().collect()
    }

    pub(crate) fn rollback_to(&mut self, checkpoint: &[(FeatureId, String)]) {
        let retained = checkpoint
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        self.registrations.retain(|key, _| retained.contains(key));
    }

    pub fn build(self) -> FeatureBackgroundTaskRegistry {
        FeatureBackgroundTaskRegistry {
            inner: Arc::new(RegistryInner {
                registrations: self.registrations,
                running: Mutex::new(BTreeMap::new()),
                diagnostics: Mutex::new(Vec::new()),
                next_execution_id: AtomicU64::new(1),
                session_generation: AtomicU64::new(1),
                accepting: AtomicBool::new(true),
            }),
        }
    }
}

struct RunningTask {
    feature_id: FeatureId,
    task_name: String,
    cancellation: BackgroundTaskCancellation,
    handle: JoinHandle<()>,
}

struct RegistryInner {
    registrations: BTreeMap<(FeatureId, String), Registration>,
    running: Mutex<BTreeMap<u64, RunningTask>>,
    diagnostics: Mutex<Vec<BackgroundTaskDiagnostic>>,
    next_execution_id: AtomicU64,
    session_generation: AtomicU64,
    accepting: AtomicBool,
}

impl Drop for RegistryInner {
    fn drop(&mut self) {
        if let Ok(running) = self.running.get_mut() {
            for task in running.values() {
                task.cancellation.cancel();
                task.handle.abort();
            }
            running.clear();
        }
    }
}

#[derive(Clone)]
pub struct FeatureBackgroundTaskRegistry {
    inner: Arc<RegistryInner>,
}

impl Default for FeatureBackgroundTaskRegistry {
    fn default() -> Self {
        FeatureBackgroundTaskRegistryBuilder::default().build()
    }
}

impl std::fmt::Debug for FeatureBackgroundTaskRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FeatureBackgroundTaskRegistry")
            .field(
                "registrations",
                &self.inner.registrations.keys().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl PartialEq for FeatureBackgroundTaskRegistry {
    fn eq(&self, other: &Self) -> bool {
        self.inner
            .registrations
            .keys()
            .eq(other.inner.registrations.keys())
    }
}

impl Eq for FeatureBackgroundTaskRegistry {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackgroundTaskStart {
    Started { execution_id: u64 },
    AtCapacity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackgroundTaskOutcome {
    Completed,
    Cancelled,
    TimedOut,
    Failed(HookError),
    JoinFailed,
    StaleGenerationDiscarded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundTaskDiagnostic {
    pub execution_id: u64,
    pub feature_id: FeatureId,
    pub task_name: String,
    pub attempts: u16,
    pub outcome: BackgroundTaskOutcome,
}

impl FeatureBackgroundTaskRegistry {
    /// Starts one execution immediately. The Worker scope intentionally owns no
    /// hidden queue: reaching `max_concurrency` returns `AtCapacity`, so callers
    /// must retain durable work in their domain authority and retry explicitly.
    pub fn start(
        &self,
        feature_id: &FeatureId,
        task_name: &str,
        invocation: HookInvocationContext,
    ) -> Result<BackgroundTaskStart, HookError> {
        if !self.inner.accepting.load(Ordering::Acquire) {
            return Err(HookError::new(
                HookErrorCategory::ScopeDisposed,
                "background task scope is stopping",
            ));
        }
        let key = (feature_id.clone(), task_name.to_string());
        let registration = self.inner.registrations.get(&key).ok_or_else(|| {
            HookError::new(
                HookErrorCategory::InvalidInput,
                format!("unknown background task `{feature_id}/{task_name}`"),
            )
        })?;

        let mut running = self
            .inner
            .running
            .lock()
            .expect("background tasks poisoned");
        running.retain(|_, running| !running.handle.is_finished());
        let active = running
            .values()
            .filter(|running| running.feature_id == *feature_id && running.task_name == task_name)
            .count();
        if active >= usize::from(registration.spec.max_concurrency) {
            return Ok(BackgroundTaskStart::AtCapacity);
        }

        let execution_id = self.inner.next_execution_id.fetch_add(1, Ordering::Relaxed);
        let session_generation = self.inner.session_generation.load(Ordering::Acquire);
        let cancellation = BackgroundTaskCancellation::default();
        let task_cancellation = cancellation.clone();
        let task = Arc::clone(&registration.task);
        let spec = registration.spec.clone();
        let feature_id = registration.feature_id.clone();
        let task_name = task_name.to_string();
        let weak_inner = Arc::downgrade(&self.inner);
        let task_feature_id = feature_id.clone();
        let task_task_name = task_name.clone();
        let handle = tokio::spawn(async move {
            let (attempts, outcome) = execute_task(
                task,
                spec,
                invocation,
                task_feature_id.clone(),
                task_task_name.clone(),
                execution_id,
                session_generation,
                task_cancellation,
            )
            .await;
            if let Some(inner) = weak_inner.upgrade() {
                let outcome =
                    if inner.session_generation.load(Ordering::Acquire) == session_generation {
                        outcome
                    } else {
                        BackgroundTaskOutcome::StaleGenerationDiscarded
                    };
                let mut diagnostics = inner.diagnostics.lock().expect("diagnostics poisoned");
                diagnostics.push(BackgroundTaskDiagnostic {
                    execution_id,
                    feature_id: task_feature_id,
                    task_name: task_task_name,
                    attempts,
                    outcome,
                });
                if diagnostics.len() > MAX_RETAINED_DIAGNOSTICS {
                    let remove = diagnostics.len() - MAX_RETAINED_DIAGNOSTICS;
                    diagnostics.drain(..remove);
                }
            }
        });
        running.insert(
            execution_id,
            RunningTask {
                feature_id,
                task_name,
                cancellation,
                handle,
            },
        );
        Ok(BackgroundTaskStart::Started { execution_id })
    }

    /// Starts every task explicitly bound to the committed-run boundary in
    /// deterministic `(FeatureId, task name)` order. Capacity is an expected
    /// single-flight outcome and leaves the already-running execution intact.
    pub fn start_run_committed(&self, invocation: HookInvocationContext) -> Result<(), HookError> {
        let tasks = self
            .inner
            .registrations
            .iter()
            .filter(|(_, registration)| {
                registration.spec.trigger == BackgroundTaskTrigger::RunCommitted
            })
            .map(|((feature_id, task_name), _)| (feature_id.clone(), task_name.clone()))
            .collect::<Vec<_>>();
        for (feature_id, task_name) in tasks {
            let _ = self.start(&feature_id, &task_name, invocation.clone())?;
        }
        Ok(())
    }

    pub fn diagnostics(&self) -> Vec<BackgroundTaskDiagnostic> {
        self.inner
            .diagnostics
            .lock()
            .expect("diagnostics poisoned")
            .clone()
    }

    pub async fn before_session_rewrite(&self) -> Result<(), HookError> {
        self.settle(false).await?;
        self.inner.session_generation.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), HookError> {
        self.inner.accepting.store(false, Ordering::Release);
        self.settle(true).await
    }

    async fn settle(&self, shutdown: bool) -> Result<(), HookError> {
        let mut waiting = Vec::new();
        {
            let mut running = self
                .inner
                .running
                .lock()
                .expect("background tasks poisoned");
            if !shutdown {
                for task in running.values() {
                    let registration = self
                        .inner
                        .registrations
                        .get(&(task.feature_id.clone(), task.task_name.clone()))
                        .expect("running background task must retain registration");
                    if registration.spec.rewrite == BackgroundTaskRewritePolicy::Block {
                        return Err(HookError::new(
                            HookErrorCategory::Dependency,
                            "session rewrite blocked by a running feature background task",
                        ));
                    }
                }
            }
            let ids = running.keys().copied().collect::<Vec<_>>();
            for id in ids {
                let Some(task) = running.remove(&id) else {
                    continue;
                };
                let registration = self
                    .inner
                    .registrations
                    .get(&(task.feature_id.clone(), task.task_name.clone()))
                    .expect("running background task must retain registration");
                let cancel = if shutdown {
                    registration.spec.shutdown == BackgroundTaskShutdownPolicy::CancelAndWait
                } else {
                    match registration.spec.rewrite {
                        BackgroundTaskRewritePolicy::CancelAndWait => true,
                        BackgroundTaskRewritePolicy::Wait => false,
                        BackgroundTaskRewritePolicy::Block => unreachable!(
                            "blocking rewrite policies are rejected before task handles are drained"
                        ),
                    }
                };
                if cancel {
                    task.cancellation.cancel();
                }
                waiting.push((
                    id,
                    task.feature_id.clone(),
                    task.task_name.clone(),
                    task.handle,
                ));
            }
        }

        let settle_deadline =
            tokio::time::Instant::now() + Duration::from_millis(TASK_SETTLE_TIMEOUT_MS);
        let mut join_failed = false;
        for (execution_id, feature_id, task_name, mut handle) in waiting {
            let outcome = match tokio::time::timeout_at(settle_deadline, &mut handle).await {
                Ok(Ok(())) => None,
                Ok(Err(_)) => {
                    join_failed = true;
                    Some(BackgroundTaskOutcome::JoinFailed)
                }
                Err(_) => {
                    join_failed = true;
                    handle.abort();
                    let _ = handle.await;
                    Some(BackgroundTaskOutcome::TimedOut)
                }
            };
            if let Some(outcome) = outcome {
                let mut diagnostics = self.inner.diagnostics.lock().expect("diagnostics poisoned");
                diagnostics.push(BackgroundTaskDiagnostic {
                    execution_id,
                    feature_id,
                    task_name,
                    attempts: 0,
                    outcome,
                });
                if diagnostics.len() > MAX_RETAINED_DIAGNOSTICS {
                    let remove = diagnostics.len() - MAX_RETAINED_DIAGNOSTICS;
                    diagnostics.drain(..remove);
                }
            }
        }
        if join_failed {
            return Err(HookError::new(
                HookErrorCategory::Internal,
                "feature background task settlement failed or exceeded its host deadline",
            ));
        }
        Ok(())
    }
}

async fn execute_task(
    task: Arc<dyn FeatureBackgroundTask>,
    spec: BackgroundTaskSpec,
    invocation: HookInvocationContext,
    feature_id: FeatureId,
    task_name: String,
    execution_id: u64,
    session_generation: u64,
    cancellation: BackgroundTaskCancellation,
) -> (u16, BackgroundTaskOutcome) {
    let (max_attempts, delay_ms) = match spec.retry {
        BackgroundTaskRetryPolicy::Never => (1, 0),
        BackgroundTaskRetryPolicy::Bounded {
            max_attempts,
            delay_ms,
        } => (max_attempts, delay_ms),
    };
    let deadline = tokio::time::Instant::now() + Duration::from_millis(spec.timeout_ms);
    for attempt in 1..=max_attempts {
        if cancellation.is_cancelled() {
            return (attempt, BackgroundTaskOutcome::Cancelled);
        }
        if tokio::time::Instant::now() >= deadline {
            return (attempt, BackgroundTaskOutcome::TimedOut);
        }
        let context = BackgroundTaskContext {
            invocation: invocation.clone(),
            feature_id: feature_id.clone(),
            task_name: task_name.clone(),
            execution_id,
            session_generation,
            attempt,
        };
        let result =
            tokio::time::timeout_at(deadline, task.run(context, cancellation.clone())).await;
        match result {
            Ok(Ok(())) => return (attempt, BackgroundTaskOutcome::Completed),
            Ok(Err(_error)) if cancellation.is_cancelled() => {
                return (attempt, BackgroundTaskOutcome::Cancelled);
            }
            Ok(Err(error)) if attempt == max_attempts => {
                return (attempt, BackgroundTaskOutcome::Failed(error));
            }
            Ok(Err(_)) => {}
            Err(_) => return (attempt, BackgroundTaskOutcome::TimedOut),
        }
        if delay_ms > 0 {
            let retry_at = std::cmp::min(
                deadline,
                tokio::time::Instant::now() + Duration::from_millis(delay_ms),
            );
            tokio::select! {
                () = tokio::time::sleep_until(retry_at) => {
                    if tokio::time::Instant::now() >= deadline {
                        return (attempt, BackgroundTaskOutcome::TimedOut);
                    }
                }
                () = cancellation.cancelled() => {
                    return (attempt, BackgroundTaskOutcome::Cancelled);
                }
            }
        }
    }
    (
        max_attempts,
        BackgroundTaskOutcome::Failed(HookError::new(
            HookErrorCategory::Internal,
            "background task exhausted retry policy",
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn invocation() -> HookInvocationContext {
        HookInvocationContext {
            workspace_id: Some("workspace".into()),
            worker_id: "worker".into(),
            session_id: "session".into(),
            session_revision: 3,
            run_id: Some("run".into()),
            turn_index: Some(2),
            call_id: None,
        }
    }

    struct WaitForCancellation;

    #[async_trait]
    impl FeatureBackgroundTask for WaitForCancellation {
        async fn run(
            &self,
            _context: BackgroundTaskContext,
            cancellation: BackgroundTaskCancellation,
        ) -> Result<(), HookError> {
            cancellation.cancelled().await;
            Err(HookError::new(HookErrorCategory::Cancelled, "cancelled"))
        }
    }

    #[tokio::test]
    async fn single_flight_rejects_overlap_and_shutdown_joins_the_task() {
        let feature = FeatureId::builtin("background-test");
        let declaration = BackgroundTaskDeclaration::worker_managed("extract", "extract");
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(
                feature.clone(),
                BackgroundTaskSpec::single_flight(declaration, Duration::from_secs(1)),
                WaitForCancellation,
            )
            .unwrap();
        let registry = builder.build();

        assert!(matches!(
            registry.start(&feature, "extract", invocation()).unwrap(),
            BackgroundTaskStart::Started { .. }
        ));
        assert_eq!(
            registry.start(&feature, "extract", invocation()).unwrap(),
            BackgroundTaskStart::AtCapacity
        );

        registry.shutdown().await.unwrap();
        assert_eq!(registry.diagnostics().len(), 1);
        assert_eq!(
            registry.diagnostics()[0].outcome,
            BackgroundTaskOutcome::Cancelled
        );
        assert!(matches!(
            registry.start(&feature, "extract", invocation()),
            Err(HookError {
                category: HookErrorCategory::ScopeDisposed,
                ..
            })
        ));
    }

    struct FailTwice {
        calls: Arc<AtomicUsize>,
        completed: Arc<Notify>,
    }

    #[async_trait]
    impl FeatureBackgroundTask for FailTwice {
        async fn run(
            &self,
            _context: BackgroundTaskContext,
            _cancellation: BackgroundTaskCancellation,
        ) -> Result<(), HookError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call < 3 {
                return Err(HookError::new(HookErrorCategory::Dependency, "retry"));
            }
            self.completed.notify_one();
            Ok(())
        }
    }

    #[tokio::test]
    async fn bounded_retry_records_attempts_without_task_output() {
        let feature = FeatureId::builtin("retry-test");
        let declaration = BackgroundTaskDeclaration::worker_managed("retry", "retry");
        let calls = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(Notify::new());
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        let mut spec = BackgroundTaskSpec::single_flight(declaration, Duration::from_secs(1));
        spec.trigger = BackgroundTaskTrigger::RunCommitted;
        spec.retry = BackgroundTaskRetryPolicy::Bounded {
            max_attempts: 3,
            delay_ms: 0,
        };
        builder
            .register(
                feature.clone(),
                spec,
                FailTwice {
                    calls: Arc::clone(&calls),
                    completed: Arc::clone(&completed),
                },
            )
            .unwrap();
        let registry = builder.build();
        registry.start_run_committed(invocation()).unwrap();
        completed.notified().await;
        registry.shutdown().await.unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(registry.diagnostics()[0].attempts, 3);
        assert_eq!(
            registry.diagnostics()[0].outcome,
            BackgroundTaskOutcome::Completed
        );
    }

    #[tokio::test]
    async fn cancellation_observed_when_cancel_races_with_wait_registration() {
        for _ in 0..100 {
            let cancellation = BackgroundTaskCancellation::default();
            let waiter = cancellation.clone();
            let task = tokio::spawn(async move {
                waiter.cancelled().await;
            });
            cancellation.cancel();
            tokio::time::timeout(Duration::from_millis(100), task)
                .await
                .expect("watch-backed cancellation must not lose the transition")
                .unwrap();
        }
    }

    struct AlwaysFails {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl FeatureBackgroundTask for AlwaysFails {
        async fn run(
            &self,
            _context: BackgroundTaskContext,
            _cancellation: BackgroundTaskCancellation,
        ) -> Result<(), HookError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(HookError::new(HookErrorCategory::Dependency, "retry"))
        }
    }

    #[tokio::test]
    async fn retry_delay_consumes_one_total_execution_deadline() {
        let feature = FeatureId::builtin("deadline-test");
        let declaration = BackgroundTaskDeclaration::worker_managed("deadline", "deadline");
        let calls = Arc::new(AtomicUsize::new(0));
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        let mut spec = BackgroundTaskSpec::single_flight(declaration, Duration::from_millis(20));
        spec.shutdown = BackgroundTaskShutdownPolicy::Wait;
        spec.retry = BackgroundTaskRetryPolicy::Bounded {
            max_attempts: 3,
            delay_ms: 100,
        };
        builder
            .register(
                feature.clone(),
                spec,
                AlwaysFails {
                    calls: Arc::clone(&calls),
                },
            )
            .unwrap();
        let registry = builder.build();
        registry.start(&feature, "deadline", invocation()).unwrap();
        registry.shutdown().await.unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            registry.diagnostics()[0].outcome,
            BackgroundTaskOutcome::TimedOut
        );
    }

    #[tokio::test]
    async fn rewrite_joins_old_tasks_before_advancing_session_generation() {
        let feature = FeatureId::builtin("generation-test");
        let declaration = BackgroundTaskDeclaration::worker_managed("generation", "generation");
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(
                feature.clone(),
                BackgroundTaskSpec::single_flight(declaration, Duration::from_secs(1)),
                WaitForCancellation,
            )
            .unwrap();
        let registry = builder.build();
        registry
            .start(&feature, "generation", invocation())
            .unwrap();
        assert_eq!(registry.inner.session_generation.load(Ordering::Acquire), 1);

        registry.before_session_rewrite().await.unwrap();

        assert_eq!(registry.inner.session_generation.load(Ordering::Acquire), 2);
        assert_eq!(registry.diagnostics().len(), 1);
        assert_eq!(
            registry.diagnostics()[0].outcome,
            BackgroundTaskOutcome::Cancelled
        );
    }

    #[tokio::test]
    async fn block_policy_fences_rewrite_without_detaching_the_task() {
        let feature = FeatureId::builtin("rewrite-test");
        let declaration = BackgroundTaskDeclaration::worker_managed("rewrite", "rewrite");
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        let mut spec = BackgroundTaskSpec::single_flight(declaration, Duration::from_secs(1));
        spec.rewrite = BackgroundTaskRewritePolicy::Block;
        builder
            .register(feature.clone(), spec, WaitForCancellation)
            .unwrap();
        let registry = builder.build();
        registry.start(&feature, "rewrite", invocation()).unwrap();

        let error = registry.before_session_rewrite().await.unwrap_err();
        assert_eq!(error.category, HookErrorCategory::Dependency);
        registry.shutdown().await.unwrap();
        assert_eq!(registry.diagnostics().len(), 1);
    }
}
