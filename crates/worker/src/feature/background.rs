//! Executable, scope-owned background tasks contributed by Worker features.
//!
//! Tasks never receive provider handles, credentials, or raw Workdir paths from
//! this registry. Callers pass only stable Worker/session provenance. The task
//! implementation obtains any additional authority through the services its
//! feature was explicitly granted at install time.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
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
const TASK_CLEANUP_TIMEOUT_MS: u64 = 5_000;
const MAX_HOST_READY_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_HOST_READY_RUN_MS: u64 = 30_000;
const TASK_SCOPE_ACCEPTING: u8 = 0;
const TASK_SCOPE_REWRITING: u8 = 1;
const TASK_SCOPE_STOPPING: u8 = 2;

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
    /// The registry repeats bounded runs after successful Controller publication.
    /// Failures pause this task until explicit activation; rewrite only resumes
    /// tasks that were still active. The interval follows completion, not start.
    HostReady {
        interval_ms: u64,
    },
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
        if let BackgroundTaskTrigger::HostReady { interval_ms } = self.trigger
            && (interval_ms == 0
                || interval_ms > MAX_HOST_READY_INTERVAL_MS
                || self.max_concurrency != 1
                || self.timeout_ms > MAX_HOST_READY_RUN_MS
                || self.retry != BackgroundTaskRetryPolicy::Never
                || self.rewrite != BackgroundTaskRewritePolicy::CancelAndWait
                || self.shutdown != BackgroundTaskShutdownPolicy::CancelAndWait)
        {
            return Err(FeatureInstallError::InvalidDescriptor(format!(
                "HostReady task `{}` requires interval_ms within 1..={MAX_HOST_READY_INTERVAL_MS}, single-flight runs of at most {MAX_HOST_READY_RUN_MS}ms, no automatic retry, and cancel-and-wait barriers",
                self.declaration.name
            )));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct BackgroundTaskGenerationFence {
    expected: u64,
    current: Arc<AtomicU64>,
}

impl BackgroundTaskGenerationFence {
    pub fn generation(&self) -> u64 {
        self.expected
    }

    pub fn ensure_current(&self) -> Result<(), HookError> {
        if self.current.load(Ordering::Acquire) == self.expected {
            Ok(())
        } else {
            Err(HookError::new(
                HookErrorCategory::Cancelled,
                "background task belongs to a stale Session generation",
            ))
        }
    }
}

impl std::fmt::Debug for BackgroundTaskGenerationFence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BackgroundTaskGenerationFence")
            .field("expected", &self.expected)
            .finish_non_exhaustive()
    }
}

impl PartialEq for BackgroundTaskGenerationFence {
    fn eq(&self, other: &Self) -> bool {
        self.expected == other.expected && Arc::ptr_eq(&self.current, &other.current)
    }
}

impl Eq for BackgroundTaskGenerationFence {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackgroundTaskContext {
    pub invocation: HookInvocationContext,
    pub feature_id: FeatureId,
    pub task_name: String,
    pub execution_id: u64,
    /// Token that must be checked by any separately granted mutation handle
    /// immediately before committing output derived by this task.
    pub generation_fence: BackgroundTaskGenerationFence,
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

    pub(crate) fn cancel(&self) {
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

    /// Runs once after this registry-owned driver stops, including cancellation
    /// and timeout, with no `run` future alive. HostReady normal completions do
    /// not stop the driver. Cleanup is awaited even when cancellation is set,
    /// with a separate five-second host deadline; implementors must make lease
    /// release cancellation-safe if their on-stop work fails or times out.
    async fn stopped(&self, _context: BackgroundTaskContext) -> Result<(), HookError> {
        Ok(())
    }
}

struct Registration {
    feature_id: FeatureId,
    spec: BackgroundTaskSpec,
    task: Arc<dyn FeatureBackgroundTask>,
    host_ready_active: AtomicBool,
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
                host_ready_active: AtomicBool::new(false),
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
                settlement: tokio::sync::Mutex::new(()),
                diagnostics: Mutex::new(Vec::new()),
                next_execution_id: AtomicU64::new(1),
                session_generation: Arc::new(AtomicU64::new(1)),
                lifecycle: AtomicU8::new(TASK_SCOPE_ACCEPTING),
                host_ready_invocation: Mutex::new(None),
                #[cfg(test)]
                start_race_probe: Mutex::new(None),
            }),
        }
    }
}

struct RunningTask {
    feature_id: FeatureId,
    task_name: String,
    cancellation: BackgroundTaskCancellation,
    force_stop: BackgroundTaskCancellation,
    handle: JoinHandle<Result<(), HookError>>,
}

struct SettlingTask {
    inner: Arc<RegistryInner>,
    execution_id: u64,
    task: Option<RunningTask>,
}

impl Drop for SettlingTask {
    fn drop(&mut self) {
        // A cancelled settlement must not detach its still-owned driver or
        // cleanup. A later rewrite/shutdown can continue joining the same task.
        if let Some(task) = self.task.take() {
            self.inner
                .running
                .lock()
                .expect("background tasks poisoned")
                .insert(self.execution_id, task);
        }
    }
}

#[cfg(test)]
struct StartRaceProbe {
    optimistic_check_passed: Arc<std::sync::Barrier>,
    resume: Arc<std::sync::Barrier>,
}

struct RegistryInner {
    registrations: BTreeMap<(FeatureId, String), Registration>,
    running: Mutex<BTreeMap<u64, RunningTask>>,
    settlement: tokio::sync::Mutex<()>,
    diagnostics: Mutex<Vec<BackgroundTaskDiagnostic>>,
    next_execution_id: AtomicU64,
    session_generation: Arc<AtomicU64>,
    lifecycle: AtomicU8,
    host_ready_invocation: Mutex<Option<HookInvocationContext>>,
    #[cfg(test)]
    start_race_probe: Mutex<Option<StartRaceProbe>>,
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

/// Holds the task registry quiescent across the entire Session rewrite. Drop
/// reopens starts only when shutdown has not moved the scope to Stopping. Only
/// completed settlement rearms HostReady tasks; cancelled preparation retains
/// joins in the registry for an explicit retry, without overlapping cleanup.
pub struct BackgroundTaskRewriteGuard {
    inner: Arc<RegistryInner>,
    settled: bool,
}

impl std::fmt::Debug for BackgroundTaskRewriteGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BackgroundTaskRewriteGuard")
            .finish_non_exhaustive()
    }
}

impl Drop for BackgroundTaskRewriteGuard {
    fn drop(&mut self) {
        if self
            .inner
            .lifecycle
            .compare_exchange(
                TASK_SCOPE_REWRITING,
                TASK_SCOPE_ACCEPTING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            let registry = FeatureBackgroundTaskRegistry {
                inner: Arc::clone(&self.inner),
            };
            if self.settled {
                registry.resume_host_ready();
            }
        }
    }
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
    CleanupFailed(HookError),
    CleanupTimedOut,
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
        if self.inner.lifecycle.load(Ordering::Acquire) != TASK_SCOPE_ACCEPTING {
            return Err(HookError::new(
                HookErrorCategory::ScopeDisposed,
                "background task scope is stopping",
            ));
        }
        #[cfg(test)]
        if let Some(probe) = self
            .inner
            .start_race_probe
            .lock()
            .expect("start race probe poisoned")
            .take()
        {
            probe.optimistic_check_passed.wait();
            probe.resume.wait();
        }
        let key = (feature_id.clone(), task_name.to_string());
        let registration = self.inner.registrations.get(&key).ok_or_else(|| {
            HookError::new(
                HookErrorCategory::InvalidInput,
                format!("unknown background task `{feature_id}/{task_name}`"),
            )
        })?;
        if matches!(
            registration.spec.trigger,
            BackgroundTaskTrigger::HostReady { .. }
        ) && self
            .inner
            .host_ready_invocation
            .lock()
            .expect("HostReady context poisoned")
            .is_none()
        {
            return Err(HookError::new(
                HookErrorCategory::Dependency,
                "Controller has not published Host readiness",
            ));
        }

        let mut running = self
            .inner
            .running
            .lock()
            .expect("background tasks poisoned");
        // `shutdown()` and `begin_session_rewrite()` change lifecycle before
        // taking this same lock. Recheck while holding the spawn/drain
        // serialization boundary so a starter that passed the optimistic
        // check cannot publish after either barrier.
        if self.inner.lifecycle.load(Ordering::Acquire) != TASK_SCOPE_ACCEPTING {
            return Err(HookError::new(
                HookErrorCategory::ScopeDisposed,
                "background task scope is stopping",
            ));
        }
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
        let generation_fence = BackgroundTaskGenerationFence {
            expected: session_generation,
            current: Arc::clone(&self.inner.session_generation),
        };
        let cancellation = BackgroundTaskCancellation::default();
        let task_cancellation = cancellation.clone();
        let force_stop = BackgroundTaskCancellation::default();
        let task_force_stop = force_stop.clone();
        let task = Arc::clone(&registration.task);
        let spec = registration.spec.clone();
        let feature_id = registration.feature_id.clone();
        let task_name = task_name.to_string();
        let weak_inner = Arc::downgrade(&self.inner);
        let task_feature_id = feature_id.clone();
        let task_task_name = task_name.clone();
        if matches!(spec.trigger, BackgroundTaskTrigger::HostReady { .. }) {
            registration
                .host_ready_active
                .store(true, Ordering::Release);
        }
        let handle = tokio::spawn(async move {
            let mut attempts;
            let mut outcome;
            loop {
                (attempts, outcome) = execute_task(
                    Arc::clone(&task),
                    spec.clone(),
                    invocation.clone(),
                    task_feature_id.clone(),
                    task_task_name.clone(),
                    execution_id,
                    generation_fence.clone(),
                    task_cancellation.clone(),
                    task_force_stop.clone(),
                )
                .await;
                let BackgroundTaskTrigger::HostReady { interval_ms } = spec.trigger else {
                    break;
                };
                if outcome != BackgroundTaskOutcome::Completed || task_cancellation.is_cancelled() {
                    break;
                }
                tokio::select! {
                    () = task_cancellation.cancelled() => {
                        outcome = BackgroundTaskOutcome::Cancelled;
                        break;
                    }
                    () = tokio::time::sleep(Duration::from_millis(interval_ms)) => {}
                }
            }
            // execute_task drops the run future before returning. Cleanup must
            // not be selected against the already-cancelled driver token.
            let context = BackgroundTaskContext {
                invocation,
                feature_id: task_feature_id.clone(),
                task_name: task_task_name.clone(),
                execution_id,
                generation_fence: generation_fence.clone(),
                attempt: attempts,
            };
            let cleanup = tokio::time::timeout(
                Duration::from_millis(TASK_CLEANUP_TIMEOUT_MS),
                task.stopped(context),
            )
            .await;
            let cleanup_error = match cleanup {
                Ok(Ok(())) => None,
                Ok(Err(error)) => {
                    outcome = BackgroundTaskOutcome::CleanupFailed(error.clone());
                    Some(error)
                }
                Err(_) => {
                    outcome = BackgroundTaskOutcome::CleanupTimedOut;
                    Some(HookError::new(
                        HookErrorCategory::Timeout,
                        "background task cleanup exceeded its host deadline",
                    ))
                }
            };
            if let Some(inner) = weak_inner.upgrade() {
                if matches!(spec.trigger, BackgroundTaskTrigger::HostReady { .. })
                    && !matches!(
                        outcome,
                        BackgroundTaskOutcome::Completed | BackgroundTaskOutcome::Cancelled
                    )
                {
                    inner
                        .registrations
                        .get(&(task_feature_id.clone(), task_task_name.clone()))
                        .expect("running task retains registration")
                        .host_ready_active
                        .store(false, Ordering::Release);
                }
                let outcome = if generation_fence.ensure_current().is_ok() {
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
            cleanup_error.map_or(Ok(()), Err)
        });
        running.insert(
            execution_id,
            RunningTask {
                feature_id,
                task_name,
                cancellation,
                force_stop,
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

    /// Called only after required feature installation and Controller publication
    /// succeed, for both startup and restore. Repeated publication is idempotent:
    /// paused tasks require task-key-specific reactivation, never a global rearm.
    pub fn start_host_ready(&self, invocation: HookInvocationContext) -> Result<(), HookError> {
        if self.inner.lifecycle.load(Ordering::Acquire) != TASK_SCOPE_ACCEPTING {
            return Err(HookError::new(
                HookErrorCategory::ScopeDisposed,
                "background task scope is stopping",
            ));
        }
        {
            let mut published = self
                .inner
                .host_ready_invocation
                .lock()
                .expect("HostReady context poisoned");
            if published.is_some() {
                return Ok(());
            }
            *published = Some(invocation.clone());
        }
        for ((feature_id, task_name), registration) in &self.inner.registrations {
            if matches!(
                registration.spec.trigger,
                BackgroundTaskTrigger::HostReady { .. }
            ) {
                let _ = self.start(feature_id, task_name, invocation.clone())?;
            }
        }
        Ok(())
    }

    /// Explicitly reactivates only this HostReady task after domain management
    /// has authorized resume. Readiness, rewrite/shutdown admission and the
    /// existing singleflight limit still apply. AtCapacity does not queue a wake:
    /// callers must retry if the previous driver's cleanup is not yet joined.
    pub fn reactivate_host_ready(
        &self,
        feature_id: &FeatureId,
        task_name: &str,
    ) -> Result<BackgroundTaskStart, HookError> {
        let registration = self
            .inner
            .registrations
            .get(&(feature_id.clone(), task_name.to_owned()))
            .filter(|registration| {
                matches!(
                    registration.spec.trigger,
                    BackgroundTaskTrigger::HostReady { .. }
                )
            })
            .ok_or_else(|| {
                HookError::new(
                    HookErrorCategory::InvalidInput,
                    format!("unknown HostReady task `{feature_id}/{task_name}`"),
                )
            })?;
        let invocation = self
            .inner
            .host_ready_invocation
            .lock()
            .expect("HostReady context poisoned")
            .clone()
            .ok_or_else(|| {
                HookError::new(
                    HookErrorCategory::Dependency,
                    "Controller has not published Host readiness",
                )
            })?;
        self.start(&registration.feature_id, task_name, invocation)
    }

    fn resume_host_ready(&self) {
        let invocation = self
            .inner
            .host_ready_invocation
            .lock()
            .expect("HostReady context poisoned")
            .clone();
        let Some(invocation) = invocation else {
            return;
        };
        for ((feature_id, task_name), registration) in &self.inner.registrations {
            if registration.host_ready_active.load(Ordering::Acquire)
                && let Err(error) = self.start(feature_id, task_name, invocation.clone())
            {
                // Shutdown may win after the guard released admission.
                tracing::debug!(error = %error, "HostReady task not rearmed");
            }
        }
    }

    pub fn diagnostics(&self) -> Vec<BackgroundTaskDiagnostic> {
        self.inner
            .diagnostics
            .lock()
            .expect("diagnostics poisoned")
            .clone()
    }

    pub async fn begin_session_rewrite(&self) -> Result<BackgroundTaskRewriteGuard, HookError> {
        match self.inner.lifecycle.compare_exchange(
            TASK_SCOPE_ACCEPTING,
            TASK_SCOPE_REWRITING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {}
            Err(TASK_SCOPE_REWRITING) => {
                return Err(HookError::new(
                    HookErrorCategory::Dependency,
                    "another Session rewrite already owns the background task barrier",
                ));
            }
            Err(_) => {
                return Err(HookError::new(
                    HookErrorCategory::ScopeDisposed,
                    "background task scope is stopping",
                ));
            }
        }
        let mut guard = BackgroundTaskRewriteGuard {
            inner: Arc::clone(&self.inner),
            settled: false,
        };
        let settlement = self.settle(false).await;
        guard.settled = true;
        settlement?;
        if self.inner.lifecycle.load(Ordering::Acquire) != TASK_SCOPE_REWRITING {
            return Err(HookError::new(
                HookErrorCategory::ScopeDisposed,
                "background task scope stopped during Session rewrite preparation",
            ));
        }
        self.inner.session_generation.fetch_add(1, Ordering::AcqRel);
        Ok(guard)
    }

    pub async fn shutdown(&self) -> Result<(), HookError> {
        self.inner
            .lifecycle
            .store(TASK_SCOPE_STOPPING, Ordering::Release);
        self.settle(true).await
    }

    async fn settle(&self, shutdown: bool) -> Result<(), HookError> {
        let _settlement = self.inner.settlement.lock().await;
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
                waiting.push(SettlingTask {
                    inner: Arc::clone(&self.inner),
                    execution_id: id,
                    task: Some(task),
                });
            }
        }

        let settle_deadline =
            tokio::time::Instant::now() + Duration::from_millis(TASK_SETTLE_TIMEOUT_MS);
        let mut join_failed = false;
        let cancellations = waiting
            .iter()
            .map(|waiting| {
                waiting
                    .task
                    .as_ref()
                    .expect("settling task retained")
                    .force_stop
                    .clone()
            })
            .collect::<Vec<_>>();
        for mut waiting in waiting {
            let task = waiting.task.as_mut().expect("settling task retained");
            let outcome = match tokio::time::timeout_at(settle_deadline, &mut task.handle).await {
                Ok(Ok(Ok(()))) => None,
                Ok(Ok(Err(_))) => {
                    // The runner already retained the cleanup failure diagnostic.
                    join_failed = true;
                    None
                }
                Ok(Err(_)) => {
                    join_failed = true;
                    Some(BackgroundTaskOutcome::JoinFailed)
                }
                Err(_) => {
                    join_failed = true;
                    // Stop all remaining drivers together. Cancelling the run
                    // future leaves the runner alive to await bounded cleanup;
                    // aborting its JoinHandle here would skip on-stop entirely.
                    for cancellation in &cancellations {
                        cancellation.cancel();
                    }
                    let _ = (&mut task.handle).await;
                    Some(BackgroundTaskOutcome::TimedOut)
                }
            };
            let task = waiting.task.take().expect("joined task retained");
            if let Some(outcome) = outcome {
                let mut diagnostics = self.inner.diagnostics.lock().expect("diagnostics poisoned");
                diagnostics.push(BackgroundTaskDiagnostic {
                    execution_id: waiting.execution_id,
                    feature_id: task.feature_id,
                    task_name: task.task_name,
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
    generation_fence: BackgroundTaskGenerationFence,
    cancellation: BackgroundTaskCancellation,
    force_stop: BackgroundTaskCancellation,
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
            generation_fence: generation_fence.clone(),
            attempt,
        };
        let result = tokio::select! {
            biased;
            result = tokio::time::timeout_at(deadline, task.run(context, cancellation.clone())) => result,
            () = cancellation.cancelled(), if matches!(spec.trigger, BackgroundTaskTrigger::HostReady { .. }) => return (attempt, BackgroundTaskOutcome::Cancelled),
            () = force_stop.cancelled() => return (attempt, BackgroundTaskOutcome::Cancelled),
        };
        match result {
            Ok(Ok(())) => return (attempt, BackgroundTaskOutcome::Completed),
            Ok(Err(error))
                if cancellation.is_cancelled()
                    && (!matches!(spec.trigger, BackgroundTaskTrigger::HostReady { .. })
                        || error.category == HookErrorCategory::Cancelled) =>
            {
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
                () = force_stop.cancelled() => {
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn invocation() -> HookInvocationContext {
        HookInvocationContext {
            workspace_id: Some("workspace".into()),
            worker_id: "worker".into(),
            session_id: "session".into(),

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
    async fn concurrent_start_and_shutdown_leave_no_task_after_drain() {
        for _ in 0..100 {
            let feature = FeatureId::builtin("shutdown-race");
            let declaration = BackgroundTaskDeclaration::worker_managed("race", "race");
            let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
            builder
                .register(
                    feature.clone(),
                    BackgroundTaskSpec::single_flight(declaration, Duration::from_secs(1)),
                    WaitForCancellation,
                )
                .unwrap();
            let registry = builder.build();
            let gate = Arc::new(std::sync::Barrier::new(2));
            let start_registry = registry.clone();
            let start_feature = feature.clone();
            let start_gate = Arc::clone(&gate);
            let starter = tokio::task::spawn_blocking(move || {
                start_gate.wait();
                start_registry.start(&start_feature, "race", invocation())
            });

            gate.wait();
            registry.shutdown().await.unwrap();
            let start_result = starter.await.unwrap();
            assert!(matches!(
                start_result,
                Ok(BackgroundTaskStart::Started { .. })
                    | Err(HookError {
                        category: HookErrorCategory::ScopeDisposed,
                        ..
                    })
            ));
            assert!(registry.inner.running.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn starter_paused_after_optimistic_check_is_rejected_by_rewrite_barrier() {
        let feature = FeatureId::builtin("rewrite-race");
        let declaration = BackgroundTaskDeclaration::worker_managed("race", "race");
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(
                feature.clone(),
                BackgroundTaskSpec::single_flight(declaration, Duration::from_secs(1)),
                WaitForCancellation,
            )
            .unwrap();
        let registry = builder.build();
        let optimistic_check_passed = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        *registry.inner.start_race_probe.lock().unwrap() = Some(StartRaceProbe {
            optimistic_check_passed: Arc::clone(&optimistic_check_passed),
            resume: Arc::clone(&resume),
        });
        let start_registry = registry.clone();
        let starter = tokio::task::spawn_blocking(move || {
            start_registry.start(&feature, "race", invocation())
        });

        optimistic_check_passed.wait();
        let rewrite_guard = registry.begin_session_rewrite().await.unwrap();
        resume.wait();
        let error = starter.await.unwrap().unwrap_err();

        assert_eq!(error.category, HookErrorCategory::ScopeDisposed);
        assert!(registry.inner.running.lock().unwrap().is_empty());
        drop(rewrite_guard);
        assert!(registry.inner.running.lock().unwrap().is_empty());
        registry.shutdown().await.unwrap();
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

    struct StartsTaskFromRewriteHook {
        registry: FeatureBackgroundTaskRegistry,
        feature: FeatureId,
        rejected: Arc<AtomicBool>,
    }

    #[async_trait]
    impl crate::hook::Hook<crate::hook::BeforeSessionRewrite> for StartsTaskFromRewriteHook {
        async fn call(
            &self,
            _input: &crate::hook::BeforeSessionRewriteContext,
        ) -> Result<crate::hook::BeforeSessionRewriteAction, HookError> {
            let error = self
                .registry
                .start(&self.feature, "hook-start", invocation())
                .unwrap_err();
            self.rejected.store(
                error.category == HookErrorCategory::ScopeDisposed,
                Ordering::Release,
            );
            Ok(crate::hook::BeforeSessionRewriteAction::Continue)
        }
    }

    #[tokio::test]
    async fn rewrite_hook_cannot_start_task_while_quiescence_guard_is_held() {
        let feature = FeatureId::builtin("hook-start-test");
        let declaration = BackgroundTaskDeclaration::worker_managed("hook-start", "hook-start");
        let mut task_builder = FeatureBackgroundTaskRegistryBuilder::default();
        task_builder
            .register(
                feature.clone(),
                BackgroundTaskSpec::single_flight(declaration, Duration::from_secs(1)),
                WaitForCancellation,
            )
            .unwrap();
        let tasks = task_builder.build();
        let rejected = Arc::new(AtomicBool::new(false));
        let mut hook_builder = crate::hook::HookRegistryBuilder::new();
        hook_builder
            .add_named_before_session_rewrite(
                "hook-start-test",
                crate::hook::HookExecutionPolicy::fail_closed(),
                StartsTaskFromRewriteHook {
                    registry: tasks.clone(),
                    feature,
                    rejected: Arc::clone(&rejected),
                },
            )
            .unwrap();
        let hooks = hook_builder.build();
        let guard = tasks.begin_session_rewrite().await.unwrap();
        let context = crate::hook::BeforeSessionRewriteContext {
            invocation: invocation(),
            kind: crate::hook::SessionRewriteKind::Compact,
            current_history: crate::hook::HookHistoryRange::default(),
        };

        assert_eq!(
            hooks.before_session_rewrite(&context).await.unwrap(),
            crate::hook::BeforeSessionRewriteAction::Continue
        );
        assert!(rejected.load(Ordering::Acquire));
        drop(guard);
        tasks.shutdown().await.unwrap();
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
        let stale_fence = BackgroundTaskGenerationFence {
            expected: registry.inner.session_generation.load(Ordering::Acquire),
            current: Arc::clone(&registry.inner.session_generation),
        };
        registry
            .start(&feature, "generation", invocation())
            .unwrap();
        assert_eq!(registry.inner.session_generation.load(Ordering::Acquire), 1);

        let rewrite_guard = registry.begin_session_rewrite().await.unwrap();

        assert_eq!(registry.inner.session_generation.load(Ordering::Acquire), 2);
        assert!(matches!(
            registry.start(&feature, "generation", invocation()),
            Err(HookError {
                category: HookErrorCategory::ScopeDisposed,
                ..
            })
        ));
        assert_eq!(
            stale_fence.ensure_current().unwrap_err().category,
            HookErrorCategory::Cancelled
        );
        assert_eq!(registry.diagnostics().len(), 1);
        assert_eq!(
            registry.diagnostics()[0].outcome,
            BackgroundTaskOutcome::Cancelled
        );
        drop(rewrite_guard);
        assert!(matches!(
            registry
                .start(&feature, "generation", invocation())
                .unwrap(),
            BackgroundTaskStart::Started { .. }
        ));
        registry.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_during_rewrite_prevents_guard_drop_from_reopening_starts() {
        let registry = FeatureBackgroundTaskRegistry::default();
        let guard = registry.begin_session_rewrite().await.unwrap();
        registry.shutdown().await.unwrap();
        drop(guard);

        let error = registry
            .start(&FeatureId::builtin("missing"), "missing", invocation())
            .unwrap_err();
        assert_eq!(error.category, HookErrorCategory::ScopeDisposed);
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

        let error = registry.begin_session_rewrite().await.unwrap_err();
        assert_eq!(error.category, HookErrorCategory::Dependency);
        registry.shutdown().await.unwrap();
        assert_eq!(registry.diagnostics().len(), 1);
    }
    struct LiveRun(Arc<AtomicUsize>);

    impl Drop for LiveRun {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct ControlledTask {
        runs: tokio::sync::mpsc::UnboundedSender<BackgroundTaskContext>,
        stops: tokio::sync::mpsc::UnboundedSender<BackgroundTaskContext>,
        release_run: Arc<tokio::sync::Semaphore>,
        release_stop: Option<Arc<tokio::sync::Semaphore>>,
        live: Arc<AtomicUsize>,
        fail: Arc<AtomicBool>,
        cleanup_fails: bool,
        cleanup_dropped: Arc<AtomicBool>,
    }

    struct CleanupDrop(Arc<AtomicBool>);
    impl Drop for CleanupDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl FeatureBackgroundTask for ControlledTask {
        async fn run(
            &self,
            context: BackgroundTaskContext,
            _cancellation: BackgroundTaskCancellation,
        ) -> Result<(), HookError> {
            assert_eq!(
                self.live.fetch_add(1, Ordering::SeqCst),
                0,
                "runs must not overlap"
            );
            let _live = LiveRun(self.live.clone());
            self.runs.send(context).unwrap();
            self.release_run.acquire().await.unwrap().forget();
            if self.fail.load(Ordering::SeqCst) {
                Err(HookError::new(
                    HookErrorCategory::Dependency,
                    "consumer paused",
                ))
            } else {
                Ok(())
            }
        }

        async fn stopped(&self, context: BackgroundTaskContext) -> Result<(), HookError> {
            assert_eq!(
                self.live.load(Ordering::SeqCst),
                0,
                "on-stop cannot overlap run"
            );
            let _cleanup = CleanupDrop(self.cleanup_dropped.clone());
            self.stops.send(context).unwrap();
            if let Some(release) = &self.release_stop {
                release.acquire().await.unwrap().forget();
            }
            if self.cleanup_fails {
                Err(HookError::new(
                    HookErrorCategory::Dependency,
                    "on-stop failed",
                ))
            } else {
                Ok(())
            }
        }
    }

    fn controlled_task() -> (
        ControlledTask,
        tokio::sync::mpsc::UnboundedReceiver<BackgroundTaskContext>,
        tokio::sync::mpsc::UnboundedReceiver<BackgroundTaskContext>,
    ) {
        let (runs_tx, runs) = tokio::sync::mpsc::unbounded_channel();
        let (stops_tx, stops) = tokio::sync::mpsc::unbounded_channel();
        (
            ControlledTask {
                runs: runs_tx,
                stops: stops_tx,
                release_run: Arc::new(tokio::sync::Semaphore::new(0)),
                release_stop: None,
                live: Arc::new(AtomicUsize::new(0)),
                fail: Arc::new(AtomicBool::new(false)),
                cleanup_fails: false,
                cleanup_dropped: Arc::new(AtomicBool::new(false)),
            },
            runs,
            stops,
        )
    }

    fn host_ready_spec(name: &str) -> BackgroundTaskSpec {
        let mut spec = BackgroundTaskSpec::single_flight(
            BackgroundTaskDeclaration::worker_managed(name, name),
            Duration::from_secs(1),
        );
        spec.trigger = BackgroundTaskTrigger::HostReady { interval_ms: 1 };
        spec
    }

    async fn next_context(
        receiver: &mut tokio::sync::mpsc::UnboundedReceiver<BackgroundTaskContext>,
    ) -> BackgroundTaskContext {
        tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn host_ready_repeats_only_completed_runs_without_overlap_or_install_effects() {
        let feature = FeatureId::builtin("recurring");
        let (task, mut runs, mut stops) = controlled_task();
        let release = task.release_run.clone();
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(feature.clone(), host_ready_spec("service"), task)
            .unwrap();
        let registry = builder.build();
        registry.start_run_committed(invocation()).unwrap();
        assert!(runs.try_recv().is_err());
        assert_eq!(
            registry
                .start(&feature, "service", invocation())
                .unwrap_err()
                .category,
            HookErrorCategory::Dependency
        );
        registry.start_host_ready(invocation()).unwrap();
        let first = next_context(&mut runs).await;
        registry.start_host_ready(invocation()).unwrap();
        assert_eq!(
            registry.start(&feature, "service", invocation()).unwrap(),
            BackgroundTaskStart::AtCapacity
        );
        assert!(runs.try_recv().is_err());
        release.add_permits(1);
        let second = next_context(&mut runs).await;
        assert_eq!(first.execution_id, second.execution_id);
        assert!(
            stops.try_recv().is_err(),
            "normal completion must not dispose the driver"
        );
        registry.shutdown().await.unwrap();
        next_context(&mut stops).await;
        assert!(runs.try_recv().is_err());
        assert!(registry.inner.running.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn host_ready_failure_stays_paused_across_rewrite_until_explicit_reactivation() {
        let feature = FeatureId::builtin("paused");
        let (task, mut runs, mut stops) = controlled_task();
        let release = task.release_run.clone();
        let fail = task.fail.clone();
        fail.store(true, Ordering::SeqCst);
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(feature.clone(), host_ready_spec("service"), task)
            .unwrap();
        let registry = builder.build();
        registry.start_host_ready(invocation()).unwrap();
        let first = next_context(&mut runs).await;
        release.add_permits(1);
        next_context(&mut stops).await;
        let guard = registry.begin_session_rewrite().await.unwrap();
        assert!(matches!(
            registry.diagnostics()[0].outcome,
            BackgroundTaskOutcome::Failed(HookError {
                category: HookErrorCategory::Dependency,
                ..
            })
        ));
        drop(guard);
        assert!(registry.inner.running.lock().unwrap().is_empty());
        assert!(runs.try_recv().is_err());
        fail.store(false, Ordering::SeqCst);
        registry.reactivate_host_ready(&feature, "service").unwrap();
        let reactivated = next_context(&mut runs).await;
        assert_ne!(first.execution_id, reactivated.execution_id);
        registry.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn host_ready_rearms_only_after_full_rewrite_guard_with_fresh_generation() {
        let feature = FeatureId::builtin("rewrite-recurring");
        let (task, mut runs, mut stops) = controlled_task();
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(feature.clone(), host_ready_spec("service"), task)
            .unwrap();
        let registry = builder.build();
        registry.start_host_ready(invocation()).unwrap();
        let old = next_context(&mut runs).await;
        let guard = registry.begin_session_rewrite().await.unwrap();
        let stopped = next_context(&mut stops).await;
        assert_eq!(old.execution_id, stopped.execution_id);
        assert!(registry.inner.running.lock().unwrap().is_empty());
        assert!(runs.try_recv().is_err());
        assert_eq!(
            registry
                .start(&feature, "service", invocation())
                .unwrap_err()
                .category,
            HookErrorCategory::ScopeDisposed
        );
        assert!(old.generation_fence.ensure_current().is_err());
        // The caller still owns the guard throughout its hooks and storage rewrite.
        drop(guard);
        let fresh = next_context(&mut runs).await;
        assert_ne!(old.execution_id, fresh.execution_id);
        assert_eq!(
            fresh.generation_fence.generation(),
            old.generation_fence.generation() + 1
        );
        fresh.generation_fence.ensure_current().unwrap();
        let guard = registry.begin_session_rewrite().await.unwrap();
        registry.shutdown().await.unwrap();
        drop(guard);
        assert!(registry.inner.running.lock().unwrap().is_empty());
        assert!(runs.try_recv().is_err(), "shutdown must prevent rearm");
    }

    #[tokio::test]
    async fn shutdown_awaits_post_driver_cleanup_and_isolates_cleanup_failure() {
        let feature = FeatureId::builtin("cleanup-isolation");
        let (mut failed, mut failed_runs, mut failed_stops) = controlled_task();
        let (healthy, mut healthy_runs, mut healthy_stops) = controlled_task();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        failed.release_stop = Some(release.clone());
        failed.cleanup_fails = true;
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(feature.clone(), host_ready_spec("failed"), failed)
            .unwrap();
        builder
            .register(feature, host_ready_spec("healthy"), healthy)
            .unwrap();
        let registry = builder.build();
        registry.start_host_ready(invocation()).unwrap();
        next_context(&mut failed_runs).await;
        next_context(&mut healthy_runs).await;
        let shutdown_registry = registry.clone();
        let shutdown = tokio::spawn(async move { shutdown_registry.shutdown().await });
        next_context(&mut failed_stops).await;
        next_context(&mut healthy_stops).await;
        assert!(
            !shutdown.is_finished(),
            "join must include cleanup, not just run stop"
        );
        release.add_permits(1);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), shutdown)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        let diagnostics = registry.diagnostics();
        assert!(diagnostics.iter().any(|d| d.task_name == "failed"
            && matches!(d.outcome, BackgroundTaskOutcome::CleanupFailed(_))));
        assert!(
            diagnostics
                .iter()
                .any(|d| d.task_name == "healthy" && d.outcome == BackgroundTaskOutcome::Cancelled)
        );
        assert!(registry.inner.running.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn hung_cleanup_has_separate_host_deadline_and_drops_before_shutdown_returns() {
        let feature = FeatureId::builtin("cleanup-timeout");
        let (mut task, mut runs, mut stops) = controlled_task();
        task.release_stop = Some(Arc::new(tokio::sync::Semaphore::new(0)));
        let dropped = task.cleanup_dropped.clone();
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(feature, host_ready_spec("service"), task)
            .unwrap();
        let registry = builder.build();
        registry.start_host_ready(invocation()).unwrap();
        next_context(&mut runs).await;
        let error = tokio::time::timeout(Duration::from_secs(7), registry.shutdown())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.category, HookErrorCategory::Internal);
        next_context(&mut stops).await;
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(
            registry.diagnostics()[0].outcome,
            BackgroundTaskOutcome::CleanupTimedOut
        );
        assert!(registry.inner.running.lock().unwrap().is_empty());
    }

    #[test]
    fn host_ready_registration_rejects_unbounded_or_zero_interval_and_retry() {
        for interval_ms in [0, MAX_HOST_READY_INTERVAL_MS + 1] {
            let mut spec = host_ready_spec("service");
            spec.trigger = BackgroundTaskTrigger::HostReady { interval_ms };
            assert!(spec.validate().is_err());
        }
        let mut spec = host_ready_spec("service");
        spec.retry = BackgroundTaskRetryPolicy::Bounded {
            max_attempts: 2,
            delay_ms: 1,
        };
        assert!(spec.validate().is_err());
        let mut spec = host_ready_spec("service");
        spec.timeout_ms = MAX_HOST_READY_RUN_MS + 1;
        assert!(spec.validate().is_err());
    }
    #[tokio::test]
    async fn host_ready_run_timeout_pauses_only_that_task_and_joins_cleanup() {
        let feature = FeatureId::builtin("run-timeout-isolation");
        let (timed, mut timed_runs, mut timed_stops) = controlled_task();
        let (healthy, mut healthy_runs, _healthy_stops) = controlled_task();
        let healthy_release = healthy.release_run.clone();
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        let mut short = host_ready_spec("timed");
        short.timeout_ms = 10;
        builder.register(feature.clone(), short, timed).unwrap();
        builder
            .register(feature, host_ready_spec("healthy"), healthy)
            .unwrap();
        let registry = builder.build();
        registry.start_host_ready(invocation()).unwrap();
        next_context(&mut timed_runs).await;
        let first_healthy = next_context(&mut healthy_runs).await;
        next_context(&mut timed_stops).await;
        healthy_release.add_permits(1);
        let second_healthy = next_context(&mut healthy_runs).await;
        assert_eq!(first_healthy.execution_id, second_healthy.execution_id);
        assert!(timed_runs.try_recv().is_err());
        let guard = registry.begin_session_rewrite().await.unwrap();
        drop(guard);
        next_context(&mut healthy_runs).await;
        assert!(
            timed_runs.try_recv().is_err(),
            "rewrite cannot retry timed-out task"
        );
        assert!(
            registry
                .diagnostics()
                .iter()
                .any(|d| d.task_name == "timed" && d.outcome == BackgroundTaskOutcome::TimedOut)
        );
        registry.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_during_rewrite_waits_for_the_inflight_cleanup_without_rearming() {
        let feature = FeatureId::builtin("rewrite-shutdown-cleanup");
        let (mut task, mut runs, mut stops) = controlled_task();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        task.release_stop = Some(release.clone());
        let dropped = task.cleanup_dropped.clone();
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(feature, host_ready_spec("service"), task)
            .unwrap();
        let registry = builder.build();
        registry.start_host_ready(invocation()).unwrap();
        next_context(&mut runs).await;
        let rewriting = registry.clone();
        let rewrite = tokio::spawn(async move { rewriting.begin_session_rewrite().await });
        next_context(&mut stops).await;
        let stopping = registry.clone();
        let shutdown = tokio::spawn(async move { stopping.shutdown().await });
        // Confirm shutdown has closed admission before allowing cleanup to end.
        tokio::time::timeout(Duration::from_secs(2), async {
            while registry.inner.lifecycle.load(Ordering::Acquire) != TASK_SCOPE_STOPPING {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!shutdown.is_finished());
        assert!(!dropped.load(Ordering::SeqCst));
        release.add_permits(1);
        assert_eq!(
            rewrite.await.unwrap().unwrap_err().category,
            HookErrorCategory::ScopeDisposed
        );
        shutdown.await.unwrap().unwrap();
        assert!(dropped.load(Ordering::SeqCst));
        assert!(runs.try_recv().is_err());
        assert!(registry.inner.running.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn cancelled_rewrite_preparation_retains_cleanup_join_until_explicit_retry() {
        let feature = FeatureId::builtin("cancelled-rewrite");
        let (mut task, mut runs, mut stops) = controlled_task();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        task.release_stop = Some(release.clone());
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(feature.clone(), host_ready_spec("service"), task)
            .unwrap();
        let registry = builder.build();
        registry.start_host_ready(invocation()).unwrap();
        let old = next_context(&mut runs).await;
        let rewriting = registry.clone();
        let rewrite = tokio::spawn(async move { rewriting.begin_session_rewrite().await });
        next_context(&mut stops).await;
        rewrite.abort();
        assert!(rewrite.await.unwrap_err().is_cancelled());
        assert_eq!(
            registry.inner.running.lock().unwrap().len(),
            1,
            "cancelled join stays owned"
        );
        old.generation_fence.ensure_current().unwrap();
        assert_eq!(
            registry.start(&feature, "service", invocation()).unwrap(),
            BackgroundTaskStart::AtCapacity
        );
        assert!(runs.try_recv().is_err());
        release.add_permits(1);
        let guard = registry.begin_session_rewrite().await.unwrap();
        assert!(registry.inner.running.lock().unwrap().is_empty());
        drop(guard);
        let fresh = next_context(&mut runs).await;
        assert_eq!(
            fresh.generation_fence.generation(),
            old.generation_fence.generation() + 1
        );
        release.add_permits(1);
        registry.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn task_key_reactivation_resumes_only_selected_failure_and_respects_barriers() {
        let feature = FeatureId::builtin("targeted-reactivation");
        let (selected, mut selected_runs, mut selected_stops) = controlled_task();
        let (other, mut other_runs, mut other_stops) = controlled_task();
        let selected_fail = selected.fail.clone();
        selected_fail.store(true, Ordering::SeqCst);
        other.fail.store(true, Ordering::SeqCst);
        let selected_release = selected.release_run.clone();
        let other_release = other.release_run.clone();
        let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
        builder
            .register(feature.clone(), host_ready_spec("selected"), selected)
            .unwrap();
        builder
            .register(feature.clone(), host_ready_spec("other"), other)
            .unwrap();
        builder
            .register(
                feature.clone(),
                BackgroundTaskSpec::single_flight(
                    BackgroundTaskDeclaration::worker_managed("manual", "manual"),
                    Duration::from_secs(1),
                ),
                WaitForCancellation,
            )
            .unwrap();
        let registry = builder.build();
        assert_eq!(
            registry
                .reactivate_host_ready(&feature, "selected")
                .unwrap_err()
                .category,
            HookErrorCategory::Dependency
        );
        registry.start_host_ready(invocation()).unwrap();
        next_context(&mut selected_runs).await;
        next_context(&mut other_runs).await;
        selected_release.add_permits(1);
        other_release.add_permits(1);
        next_context(&mut selected_stops).await;
        next_context(&mut other_stops).await;
        let guard = registry.begin_session_rewrite().await.unwrap();
        assert_eq!(
            registry
                .reactivate_host_ready(&feature, "selected")
                .unwrap_err()
                .category,
            HookErrorCategory::ScopeDisposed
        );
        drop(guard);
        registry.start_host_ready(invocation()).unwrap();
        assert!(
            registry.inner.running.lock().unwrap().is_empty(),
            "repeat publication cannot globally reactivate failures"
        );
        assert_eq!(
            registry
                .reactivate_host_ready(&feature, "manual")
                .unwrap_err()
                .category,
            HookErrorCategory::InvalidInput
        );
        assert_eq!(
            registry
                .reactivate_host_ready(&feature, "unknown")
                .unwrap_err()
                .category,
            HookErrorCategory::InvalidInput
        );
        selected_fail.store(false, Ordering::SeqCst);
        assert!(matches!(
            registry
                .reactivate_host_ready(&feature, "selected")
                .unwrap(),
            BackgroundTaskStart::Started { .. }
        ));
        next_context(&mut selected_runs).await;
        assert_eq!(
            registry
                .reactivate_host_ready(&feature, "selected")
                .unwrap(),
            BackgroundTaskStart::AtCapacity
        );
        assert!(other_runs.try_recv().is_err());
        let guard = registry.begin_session_rewrite().await.unwrap();
        drop(guard);
        next_context(&mut selected_runs).await;
        assert!(
            other_runs.try_recv().is_err(),
            "only reactivated tasks may rearm after rewrite"
        );
        registry.shutdown().await.unwrap();
        assert_eq!(
            registry
                .reactivate_host_ready(&feature, "selected")
                .unwrap_err()
                .category,
            HookErrorCategory::ScopeDisposed
        );
        assert!(other_runs.try_recv().is_err());
    }
}
