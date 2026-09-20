//! Alias-keyed Worker Workdir session routing.
//!
//! The router owns live provider sessions without exposing whether a session is
//! local or transported over HTTP.  Attachment aliases are Worker-local stable
//! identities; provider Workdir ids and human display names are deliberately not
//! routing keys.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{
    CommandEvent, CommandHandle, CommandOutput, CommandOutputRequest, CommandRequest,
    CommandSnapshot, CommandStatus, EditRequest, EditResult, GlobRequest, GlobResult, GrepRequest,
    GrepResult, ListRequest, ListResult, ReadRequest, ReadResult, StatRequest, StatResult, Workdir,
    WorkdirError, WorkdirScopeAuthorizationRequest, WorkdirScopeOverlapRequest, WorkdirSession,
    WorkdirSessionCapabilities, WorkdirSessionHandle, WriteRequest, WriteResult,
};

/// Stable Worker-local routing key for one attached Workdir.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct WorkdirAttachmentAlias(String);

impl WorkdirAttachmentAlias {
    pub fn new(value: impl Into<String>) -> Result<Self, WorkdirError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 64
            && value.bytes().enumerate().all(|(index, byte)| match byte {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => true,
                b'_' | b'-' | b'.' => index > 0,
                _ => false,
            });
        if !valid {
            return Err(WorkdirError::InvalidArgument(
                "attachment alias must be 1-64 ASCII letters, digits, '.', '_' or '-', and start with a letter or digit"
                    .to_string(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for WorkdirAttachmentAlias {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl std::fmt::Display for WorkdirAttachmentAlias {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Stable machine-readable reason why a tool target could not be resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirRouteErrorCode {
    NoWorkdirAttached,
    TargetWorkdirRequired,
    UnknownTargetWorkdir,
}

/// Provider-neutral attachment routing failure returned before an operation starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkdirRouteError {
    pub code: WorkdirRouteErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_workdir: Option<String>,
    pub available_aliases: Vec<String>,
}

impl std::fmt::Display for WorkdirRouteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let encoded = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        formatter.write_str(&encoded)
    }
}

impl std::error::Error for WorkdirRouteError {}

/// One attachment selected for a single tool invocation.
#[derive(Clone)]
pub struct ResolvedWorkdirSession {
    pub alias: WorkdirAttachmentAlias,
    pub generation: u64,
    pub session: WorkdirSessionHandle,
}

impl std::fmt::Debug for ResolvedWorkdirSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedWorkdirSession")
            .field("alias", &self.alias)
            .field("generation", &self.generation)
            .field("workdir", self.session.workdir())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct RoutedSessionState {
    session: WorkdirSessionHandle,
    generation: u64,
    detached: AtomicBool,
    active_operations: AtomicUsize,
    active_commands: Mutex<HashSet<CommandHandle>>,
}

impl RoutedSessionState {
    fn enter(&self) -> Result<ActiveOperation<'_>, WorkdirError> {
        if self.detached.load(Ordering::Acquire) {
            return Err(WorkdirError::Unavailable(
                "Workdir attachment is detached".to_string(),
            ));
        }
        self.active_operations.fetch_add(1, Ordering::AcqRel);
        if self.detached.load(Ordering::Acquire) {
            self.active_operations.fetch_sub(1, Ordering::AcqRel);
            return Err(WorkdirError::Unavailable(
                "Workdir attachment is detaching".to_string(),
            ));
        }
        Ok(ActiveOperation(self))
    }

    fn is_busy(&self) -> bool {
        if self.active_operations.load(Ordering::Acquire) != 0 {
            return true;
        }
        self.reconcile_terminal_commands(&self.session.command_snapshot());
        !self
            .active_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    async fn is_busy_for_detach(&self) -> bool {
        if self.active_operations.load(Ordering::Acquire) != 0 {
            return true;
        }
        self.reconcile_terminal_commands(&self.session.command_snapshot());
        let handles = self
            .active_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        for handle in handles {
            match self.session.command_status(handle.clone()).await {
                Ok(status) => self.mark_terminal(&handle, status),
                Err(WorkdirError::UnknownCommand(_)) => {
                    self.active_commands
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&handle);
                }
                Err(_) => {}
            }
        }
        self.active_operations.load(Ordering::Acquire) != 0
            || !self
                .active_commands
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
    }

    fn reconcile_terminal_commands(&self, snapshots: &[CommandSnapshot]) {
        let terminal = snapshots
            .iter()
            .filter(|snapshot| snapshot.status != CommandStatus::Running)
            .map(|snapshot| snapshot.command_id.as_str())
            .collect::<HashSet<_>>();
        if terminal.is_empty() {
            return;
        }
        self.active_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|handle| !terminal.contains(handle.0.as_str()));
    }

    fn mark_terminal(&self, handle: &CommandHandle, status: CommandStatus) {
        if status != CommandStatus::Running {
            self.active_commands
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(handle);
        }
    }
}

struct ActiveOperation<'a>(&'a RoutedSessionState);

impl Drop for ActiveOperation<'_> {
    fn drop(&mut self) {
        self.0.active_operations.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A session handle bound to one alias in a [`WorkdirSessionRouter`].
#[derive(Debug, Clone)]
pub struct RoutedWorkdirSession {
    state: Arc<RoutedSessionState>,
}

#[async_trait]
impl WorkdirSession for RoutedWorkdirSession {
    fn workdir(&self) -> &Workdir {
        self.state.session.workdir()
    }

    fn capabilities(&self) -> WorkdirSessionCapabilities {
        self.state.session.capabilities()
    }

    async fn authorize_scope_path(
        &self,
        request: WorkdirScopeAuthorizationRequest,
    ) -> Result<(), WorkdirError> {
        let _active = self.state.enter()?;
        self.state.session.authorize_scope_path(request).await
    }

    async fn scope_rules_overlap(
        &self,
        request: WorkdirScopeOverlapRequest,
    ) -> Result<bool, WorkdirError> {
        let _active = self.state.enter()?;
        self.state.session.scope_rules_overlap(request).await
    }

    async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError> {
        let _active = self.state.enter()?;
        self.state.session.stat(request).await
    }

    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        let _active = self.state.enter()?;
        self.state.session.read(request).await
    }

    async fn write(&self, request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        let _active = self.state.enter()?;
        self.state.session.write(request).await
    }

    async fn edit(&self, request: EditRequest) -> Result<EditResult, WorkdirError> {
        let _active = self.state.enter()?;
        self.state.session.edit(request).await
    }

    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
        let _active = self.state.enter()?;
        self.state.session.list(request).await
    }

    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        let _active = self.state.enter()?;
        self.state.session.glob(request).await
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        let _active = self.state.enter()?;
        self.state.session.grep(request).await
    }

    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        let _active = self.state.enter()?;
        let handle = self.state.session.start_command(request).await?;
        self.state
            .active_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(handle.clone());
        Ok(handle)
    }

    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        let _active = self.state.enter()?;
        let status = self.state.session.command_status(handle.clone()).await?;
        self.state.mark_terminal(&handle, status);
        Ok(status)
    }

    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        let _active = self.state.enter()?;
        let handle = request.handle.clone();
        let output = self.state.session.command_output(request).await?;
        self.state.mark_terminal(&handle, output.status);
        Ok(output)
    }

    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
        let _active = self.state.enter()?;
        self.state.session.cancel_command(handle.clone()).await?;
        self.state
            .active_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&handle);
        Ok(())
    }

    fn subscribe_command_events(&self) -> Option<broadcast::Receiver<CommandEvent>> {
        self.state.session.subscribe_command_events()
    }

    fn command_snapshot(&self) -> Vec<CommandSnapshot> {
        let snapshots = self.state.session.command_snapshot();
        self.state.reconcile_terminal_commands(&snapshots);
        snapshots
    }

    async fn close(&self) -> Result<(), WorkdirError> {
        if self.state.is_busy() {
            return Err(WorkdirError::Conflict(
                "Workdir attachment has active operations or commands".to_string(),
            ));
        }
        self.state.session.close().await
    }
}

/// Worker-owned map of stable attachment aliases to provider-neutral sessions.
#[derive(Debug, Default)]
pub struct WorkdirSessionRouter {
    sessions: RwLock<BTreeMap<WorkdirAttachmentAlias, Arc<RoutedSessionState>>>,
    next_generation: AtomicU64,
}

impl WorkdirSessionRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn aliases(&self) -> Vec<WorkdirAttachmentAlias> {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    pub fn capabilities(&self) -> WorkdirSessionCapabilities {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .fold(WorkdirSessionCapabilities::EMPTY, |capabilities, state| {
                capabilities.union(state.session.capabilities())
            })
    }

    pub fn len(&self) -> usize {
        self.sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Resolve one model-visible target without exposing provider identity.
    ///
    /// Omission is accepted only for the unambiguous single-attachment case.
    /// Every error includes the current alias snapshot so callers can recover
    /// without consulting Workdir ids, display names, or transport metadata.
    pub fn resolve(
        &self,
        target_workdir: Option<&str>,
    ) -> Result<ResolvedWorkdirSession, WorkdirRouteError> {
        let sessions = self
            .sessions
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let available_aliases = sessions
            .keys()
            .map(|alias| alias.as_str().to_owned())
            .collect::<Vec<_>>();
        let selected = match target_workdir {
            Some(target) => sessions
                .iter()
                .find(|(alias, _)| alias.as_str() == target)
                .map(|(alias, state)| (alias.clone(), state.clone()))
                .ok_or_else(|| WorkdirRouteError {
                    code: WorkdirRouteErrorCode::UnknownTargetWorkdir,
                    target_workdir: Some(target.to_owned()),
                    available_aliases: available_aliases.clone(),
                })?,
            None if sessions.is_empty() => {
                return Err(WorkdirRouteError {
                    code: WorkdirRouteErrorCode::NoWorkdirAttached,
                    target_workdir: None,
                    available_aliases,
                });
            }
            None if sessions.len() > 1 => {
                return Err(WorkdirRouteError {
                    code: WorkdirRouteErrorCode::TargetWorkdirRequired,
                    target_workdir: None,
                    available_aliases,
                });
            }
            None => {
                let (alias, state) = sessions
                    .first_key_value()
                    .expect("single attachment exists after cardinality checks");
                (alias.clone(), state.clone())
            }
        };
        Ok(ResolvedWorkdirSession {
            alias: selected.0,
            generation: selected.1.generation,
            session: Arc::new(RoutedWorkdirSession { state: selected.1 }),
        })
    }

    pub fn attach(
        &self,
        alias: WorkdirAttachmentAlias,
        session: WorkdirSessionHandle,
    ) -> Result<WorkdirSessionHandle, WorkdirError> {
        let mut sessions = self.sessions.write().map_err(|_| {
            WorkdirError::Unavailable("Workdir attachment router is poisoned".to_string())
        })?;
        if sessions.contains_key(&alias) {
            return Err(WorkdirError::Conflict(format!(
                "Workdir attachment alias `{alias}` already exists"
            )));
        }
        let state = Arc::new(RoutedSessionState {
            session,
            generation: self.next_generation.fetch_add(1, Ordering::AcqRel),
            detached: AtomicBool::new(false),
            active_operations: AtomicUsize::new(0),
            active_commands: Mutex::new(HashSet::new()),
        });
        sessions.insert(alias, state.clone());
        Ok(Arc::new(RoutedWorkdirSession { state }))
    }

    pub fn session(&self, alias: &WorkdirAttachmentAlias) -> Option<WorkdirSessionHandle> {
        self.sessions
            .read()
            .ok()?
            .get(alias)
            .cloned()
            .map(|state| Arc::new(RoutedWorkdirSession { state }) as WorkdirSessionHandle)
    }

    /// Compatibility projection used only while filesystem tool schemas have no
    /// `target_workdir`: exactly one attachment is unambiguous; zero or multiple
    /// attachments expose no implicit primary.
    pub fn only_session(&self) -> Option<WorkdirSessionHandle> {
        let sessions = self.sessions.read().ok()?;
        if sessions.len() != 1 {
            return None;
        }
        sessions
            .values()
            .next()
            .cloned()
            .map(|state| Arc::new(RoutedWorkdirSession { state }) as WorkdirSessionHandle)
    }

    pub fn can_detach(&self, alias: &WorkdirAttachmentAlias) -> Result<(), WorkdirError> {
        let state = self
            .sessions
            .read()
            .map_err(|_| {
                WorkdirError::Unavailable("Workdir attachment router is poisoned".to_string())
            })?
            .get(alias)
            .cloned()
            .ok_or_else(|| WorkdirError::NotFound(alias.as_str().into()))?;
        if state.detached.load(Ordering::Acquire) {
            return Err(WorkdirError::Unavailable(
                "Workdir attachment is already detaching".to_string(),
            ));
        }
        if state.is_busy() {
            return Err(WorkdirError::Conflict(format!(
                "Workdir attachment `{alias}` has active operations or commands"
            )));
        }
        Ok(())
    }

    /// Atomically fence new operations before an external detach transaction begins.
    pub async fn begin_detach(&self, alias: &WorkdirAttachmentAlias) -> Result<(), WorkdirError> {
        let state = self
            .sessions
            .read()
            .map_err(|_| {
                WorkdirError::Unavailable("Workdir attachment router is poisoned".to_string())
            })?
            .get(alias)
            .cloned()
            .ok_or_else(|| WorkdirError::NotFound(alias.as_str().into()))?;
        if state.detached.swap(true, Ordering::AcqRel) {
            return Err(WorkdirError::Unavailable(
                "Workdir attachment is already detaching".to_string(),
            ));
        }
        if state.is_busy_for_detach().await {
            state.detached.store(false, Ordering::Release);
            return Err(WorkdirError::Conflict(format!(
                "Workdir attachment `{alias}` has active operations or commands"
            )));
        }
        Ok(())
    }

    /// Reopen a route after the external detach transaction failed before commit.
    pub fn cancel_detach(&self, alias: &WorkdirAttachmentAlias) {
        if let Ok(sessions) = self.sessions.read()
            && let Some(state) = sessions.get(alias)
        {
            state.detached.store(false, Ordering::Release);
        }
    }

    pub async fn finish_detach(
        &self,
        alias: &WorkdirAttachmentAlias,
    ) -> Result<WorkdirSessionHandle, WorkdirError> {
        let state = {
            let sessions = self.sessions.read().map_err(|_| {
                WorkdirError::Unavailable("Workdir attachment router is poisoned".to_string())
            })?;
            sessions
                .get(alias)
                .cloned()
                .ok_or_else(|| WorkdirError::NotFound(alias.as_str().into()))?
        };
        if !state.detached.load(Ordering::Acquire) {
            return Err(WorkdirError::Conflict(format!(
                "Workdir attachment `{alias}` was not prepared for detach"
            )));
        }
        if state.is_busy_for_detach().await {
            state.detached.store(false, Ordering::Release);
            return Err(WorkdirError::Conflict(format!(
                "Workdir attachment `{alias}` has active operations or commands"
            )));
        }
        if let Err(error) = state.session.close().await {
            state.detached.store(false, Ordering::Release);
            return Err(error);
        }
        let mut sessions = self.sessions.write().map_err(|_| {
            WorkdirError::Unavailable("Workdir attachment router is poisoned".to_string())
        })?;
        if sessions
            .get(alias)
            .is_some_and(|current| Arc::ptr_eq(current, &state))
        {
            sessions.remove(alias);
        }
        Ok(state.session.clone())
    }

    pub async fn detach(
        &self,
        alias: &WorkdirAttachmentAlias,
    ) -> Result<WorkdirSessionHandle, WorkdirError> {
        self.begin_detach(alias).await?;
        self.finish_detach(alias).await
    }

    /// Close every attachment during Worker shutdown or startup rollback.
    ///
    /// Unlike [`Self::detach`], lifecycle teardown is authoritative and delegates
    /// cancellation or draining of active commands to each provider session.
    pub async fn close_all(&self) -> Result<(), WorkdirError> {
        let aliases = self.aliases();
        for alias in aliases {
            let state = {
                let sessions = self.sessions.read().map_err(|_| {
                    WorkdirError::Unavailable("Workdir attachment router is poisoned".to_string())
                })?;
                sessions.get(&alias).cloned()
            };
            let Some(state) = state else {
                continue;
            };
            state.detached.store(true, Ordering::Release);
            if let Err(error) = state.session.close().await {
                state.detached.store(false, Ordering::Release);
                return Err(error);
            }
            let mut sessions = self.sessions.write().map_err(|_| {
                WorkdirError::Unavailable("Workdir attachment router is poisoned".to_string())
            })?;
            if sessions
                .get(&alias)
                .is_some_and(|current| Arc::ptr_eq(current, &state))
            {
                sessions.remove(&alias);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalWorkdirSession;
    use manifest::Scope;
    use tempfile::TempDir;

    fn session(id: &str, dir: &TempDir) -> WorkdirSessionHandle {
        Arc::new(LocalWorkdirSession::materialized_bound(
            Workdir::new(id),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            manifest::SharedScope::new(Scope::writable(dir.path()).unwrap()),
            WorkdirSessionCapabilities::ALL,
        ))
    }

    #[tokio::test]
    async fn router_holds_multiple_aliases_without_implicit_primary() {
        let left = TempDir::new().unwrap();
        let right = TempDir::new().unwrap();
        let router = WorkdirSessionRouter::new();
        router
            .attach(
                WorkdirAttachmentAlias::new("left").unwrap(),
                session("wd-left", &left),
            )
            .unwrap();
        router
            .attach(
                WorkdirAttachmentAlias::new("right").unwrap(),
                session("wd-right", &right),
            )
            .unwrap();
        assert_eq!(
            router
                .aliases()
                .iter()
                .map(WorkdirAttachmentAlias::as_str)
                .collect::<Vec<_>>(),
            ["left", "right"]
        );
        assert!(router.only_session().is_none());
        let missing = router.resolve(None).unwrap_err();
        assert_eq!(missing.code, WorkdirRouteErrorCode::TargetWorkdirRequired);
        assert_eq!(missing.available_aliases, ["left", "right"]);
        assert_eq!(
            router
                .resolve(Some("right"))
                .unwrap()
                .session
                .workdir()
                .id()
                .as_str(),
            "wd-right"
        );
        let unknown = router.resolve(Some("display-name")).unwrap_err();
        assert_eq!(unknown.code, WorkdirRouteErrorCode::UnknownTargetWorkdir);
        assert_eq!(unknown.target_workdir.as_deref(), Some("display-name"));
        assert_eq!(unknown.available_aliases, ["left", "right"]);
        router.close_all().await.unwrap();
    }

    #[test]
    fn empty_router_returns_structured_no_attachment_error() {
        let error = WorkdirSessionRouter::new().resolve(None).unwrap_err();
        assert_eq!(error.code, WorkdirRouteErrorCode::NoWorkdirAttached);
        assert!(error.available_aliases.is_empty());
        assert_eq!(
            serde_json::to_value(error).unwrap(),
            serde_json::json!({
                "code": "no_workdir_attached",
                "available_aliases": []
            })
        );
    }

    #[tokio::test]
    async fn detach_allows_naturally_completed_command_without_final_poll() {
        let dir = TempDir::new().unwrap();
        let router = WorkdirSessionRouter::new();
        let alias = WorkdirAttachmentAlias::new("checkout").unwrap();
        let underlying = session("wd", &dir);
        let routed = router
            .attach(alias.clone(), Arc::clone(&underlying))
            .unwrap();
        let handle = routed
            .start_command(CommandRequest {
                command: "printf done".to_string(),
                timeout_secs: 5,
                output_limit: 1024,
                cwd: None,
                spill_dir: None,
                tool_call_id: None,
            })
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let completed = underlying.command_snapshot().iter().any(|snapshot| {
                snapshot.command_id == handle.0 && snapshot.status != CommandStatus::Running
            });
            if completed {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        router.detach(&alias).await.unwrap();
        assert!(router.is_empty());
    }

    #[tokio::test]
    async fn lifecycle_close_all_closes_sessions_with_active_commands() {
        let dir = TempDir::new().unwrap();
        let router = WorkdirSessionRouter::new();
        let alias = WorkdirAttachmentAlias::new("checkout").unwrap();
        let underlying = session("wd", &dir);
        let routed = router.attach(alias, Arc::clone(&underlying)).unwrap();
        let handle = routed
            .start_command(CommandRequest {
                command: "sleep 30".to_string(),
                timeout_secs: 60,
                output_limit: 1024,
                cwd: None,
                spill_dir: None,
                tool_call_id: None,
            })
            .await
            .unwrap();

        router.close_all().await.unwrap();

        assert!(router.is_empty());
        assert!(matches!(
            underlying.command_status(handle).await,
            Err(WorkdirError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn detach_rejects_active_command_until_cancelled() {
        let dir = TempDir::new().unwrap();
        let router = WorkdirSessionRouter::new();
        let alias = WorkdirAttachmentAlias::new("checkout").unwrap();
        let routed = router.attach(alias.clone(), session("wd", &dir)).unwrap();
        let handle = routed
            .start_command(CommandRequest {
                command: "sleep 30".to_string(),
                timeout_secs: 60,
                output_limit: 1024,
                cwd: None,
                spill_dir: None,
                tool_call_id: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            router.detach(&alias).await,
            Err(WorkdirError::Conflict(_))
        ));
        routed.cancel_command(handle).await.unwrap();
        router.detach(&alias).await.unwrap();
        assert!(router.is_empty());
    }
}
