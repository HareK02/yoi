use std::{fmt, path::PathBuf};

use crate::{BackendRuntimeListTarget, BackendRuntimeTarget, WorkerRuntimeCommand};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// Legacy local Runtime process/socket authority.
    Local,
    /// One-process Standalone authority with no Runtime or Workspace backend.
    Standalone,
    Backend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedTarget {
    Local,
    Standalone,
    Backend {
        base_url: String,
        workspace_id: String,
    },
}

impl ResolvedTarget {
    pub fn kind(&self) -> TargetKind {
        match self {
            Self::Local => TargetKind::Local,
            Self::Standalone => TargetKind::Standalone,
            Self::Backend { .. } => TargetKind::Backend,
        }
    }
}

impl fmt::Display for TargetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => f.write_str("local"),
            Self::Standalone => f.write_str("Standalone"),
            Self::Backend => f.write_str("Backend"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTarget;

impl LocalTarget {
    pub fn new() -> Self {
        Self
    }

    fn runtime_command(&self) -> Result<WorkerRuntimeCommand, TargetError> {
        WorkerRuntimeCommand::resolve().map_err(TargetError::local_runtime_command)
    }
}

impl Default for LocalTarget {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendTarget {
    pub base_url: String,
    pub workspace_id: Option<String>,
}

impl BackendTarget {
    pub fn new(base_url: impl Into<String>, workspace_id: Option<impl Into<String>>) -> Self {
        Self {
            base_url: base_url.into(),
            workspace_id: workspace_id.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerListRequest {
    pub runtime_id: Option<String>,
    pub include_stopped: bool,
}

impl WorkerListRequest {
    pub fn new(runtime_id: Option<String>) -> Self {
        Self {
            runtime_id,
            include_stopped: false,
        }
    }

    pub fn with_stopped(runtime_id: Option<String>) -> Self {
        Self {
            runtime_id,
            include_stopped: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConnectionSelector {
    pub runtime_id: String,
    pub worker_id: String,
}

impl WorkerConnectionSelector {
    pub fn new(runtime_id: impl Into<String>, worker_id: impl Into<String>) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            worker_id: worker_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerSpawn {
    LegacyLocal {
        runtime_command: WorkerRuntimeCommand,
    },
    Standalone {
        state_dir: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerByName {
    pub runtime_command: WorkerRuntimeCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerResume {
    pub runtime_command: WorkerRuntimeCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandaloneSessionListIntent {
    pub state_dir: PathBuf,
    pub cwd: PathBuf,
    pub include_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandaloneSessionResumeIntent {
    pub state_dir: PathBuf,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dashboard {
    Local {
        runtime_command: WorkerRuntimeCommand,
    },
    Backend {
        base_url: String,
        workspace_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerList {
    pub local_runtime_command: Option<WorkerRuntimeCommand>,
    pub backend_target: Option<BackendRuntimeListTarget>,
    pub include_stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConnection {
    pub target: BackendRuntimeTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetError {
    message: String,
}

impl TargetError {
    fn unsupported(operation: &'static str, target: TargetKind) -> Self {
        Self {
            message: format!("{operation} is not supported by {target} target"),
        }
    }

    fn invalid(target: TargetKind, message: impl Into<String>) -> Self {
        Self {
            message: format!("invalid {target} target: {}", message.into()),
        }
    }

    fn local_runtime_command(error: std::io::Error) -> Self {
        Self {
            message: format!("failed to resolve local Worker runtime command: {error}"),
        }
    }
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TargetError {}

pub trait Target: fmt::Debug + Send + Sync {
    fn kind(&self) -> TargetKind;

    /// Resolve the target once for Workspace product-state operations.
    ///
    /// Backend targets must carry an explicit Workspace identity. Callers use
    /// this value instead of rediscovering Backend/local authority from cwd or
    /// process configuration after command dispatch.
    fn resolve(&self) -> Result<ResolvedTarget, TargetError>;

    fn spawn_worker(&self) -> Result<WorkerSpawn, TargetError>;

    fn worker_by_name(&self) -> Result<WorkerByName, TargetError>;

    fn resume_worker(&self) -> Result<WorkerResume, TargetError>;

    fn standalone_session_list(
        &self,
        _include_all: bool,
    ) -> Result<StandaloneSessionListIntent, TargetError> {
        Err(TargetError::unsupported(
            "standalone session listing",
            self.kind(),
        ))
    }

    fn standalone_session_resume(
        &self,
        _session_id: String,
    ) -> Result<StandaloneSessionResumeIntent, TargetError> {
        Err(TargetError::unsupported(
            "standalone session restore",
            self.kind(),
        ))
    }

    fn dashboard(&self) -> Result<Dashboard, TargetError>;

    fn list_workers(&self, request: WorkerListRequest) -> Result<WorkerList, TargetError>;

    fn connect_worker(
        &self,
        selector: WorkerConnectionSelector,
    ) -> Result<WorkerConnection, TargetError>;
}

impl Target for LocalTarget {
    fn kind(&self) -> TargetKind {
        TargetKind::Local
    }

    fn resolve(&self) -> Result<ResolvedTarget, TargetError> {
        Ok(ResolvedTarget::Local)
    }

    fn spawn_worker(&self) -> Result<WorkerSpawn, TargetError> {
        Ok(WorkerSpawn::LegacyLocal {
            runtime_command: self.runtime_command()?,
        })
    }

    fn worker_by_name(&self) -> Result<WorkerByName, TargetError> {
        Ok(WorkerByName {
            runtime_command: self.runtime_command()?,
        })
    }

    fn resume_worker(&self) -> Result<WorkerResume, TargetError> {
        Ok(WorkerResume {
            runtime_command: self.runtime_command()?,
        })
    }

    fn dashboard(&self) -> Result<Dashboard, TargetError> {
        Ok(Dashboard::Local {
            runtime_command: self.runtime_command()?,
        })
    }

    fn list_workers(&self, request: WorkerListRequest) -> Result<WorkerList, TargetError> {
        if request.runtime_id.is_some() {
            return Err(TargetError::unsupported(
                "Explicit runtime id for local worker listing",
                self.kind(),
            ));
        }
        Ok(WorkerList {
            local_runtime_command: Some(self.runtime_command()?),
            backend_target: None,
            include_stopped: request.include_stopped,
        })
    }

    fn connect_worker(
        &self,
        _selector: WorkerConnectionSelector,
    ) -> Result<WorkerConnection, TargetError> {
        Err(TargetError::unsupported(
            "Backend runtime worker connection",
            self.kind(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandaloneTarget {
    state_dir: PathBuf,
}

impl StandaloneTarget {
    #[must_use]
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: state_dir.into(),
        }
    }
}

impl Target for StandaloneTarget {
    fn kind(&self) -> TargetKind {
        TargetKind::Standalone
    }

    fn resolve(&self) -> Result<ResolvedTarget, TargetError> {
        Ok(ResolvedTarget::Standalone)
    }

    fn spawn_worker(&self) -> Result<WorkerSpawn, TargetError> {
        Ok(WorkerSpawn::Standalone {
            state_dir: self.state_dir.clone(),
        })
    }

    fn worker_by_name(&self) -> Result<WorkerByName, TargetError> {
        Err(TargetError::unsupported(
            "Worker name attachment",
            self.kind(),
        ))
    }

    fn resume_worker(&self) -> Result<WorkerResume, TargetError> {
        Err(TargetError::unsupported("Worker restore", self.kind()))
    }

    fn standalone_session_list(
        &self,
        include_all: bool,
    ) -> Result<StandaloneSessionListIntent, TargetError> {
        let cwd = std::env::current_dir()
            .map_err(|error| TargetError::invalid(self.kind(), error.to_string()))?;
        Ok(StandaloneSessionListIntent {
            state_dir: self.state_dir.clone(),
            cwd,
            include_all,
        })
    }

    fn standalone_session_resume(
        &self,
        session_id: String,
    ) -> Result<StandaloneSessionResumeIntent, TargetError> {
        Ok(StandaloneSessionResumeIntent {
            state_dir: self.state_dir.clone(),
            session_id,
        })
    }

    fn dashboard(&self) -> Result<Dashboard, TargetError> {
        Err(TargetError::unsupported("Worker dashboard", self.kind()))
    }

    fn list_workers(&self, _request: WorkerListRequest) -> Result<WorkerList, TargetError> {
        Err(TargetError::unsupported("Worker listing", self.kind()))
    }

    fn connect_worker(
        &self,
        _selector: WorkerConnectionSelector,
    ) -> Result<WorkerConnection, TargetError> {
        Err(TargetError::unsupported(
            "Backend runtime worker connection",
            self.kind(),
        ))
    }
}

impl Target for BackendTarget {
    fn kind(&self) -> TargetKind {
        TargetKind::Backend
    }

    fn resolve(&self) -> Result<ResolvedTarget, TargetError> {
        let workspace_id = self.workspace_id.clone().ok_or_else(|| {
            TargetError::invalid(
                self.kind(),
                "workspace selection is required for Backend product-state operations",
            )
        })?;
        Ok(ResolvedTarget::Backend {
            base_url: self.base_url.clone(),
            workspace_id,
        })
    }

    fn spawn_worker(&self) -> Result<WorkerSpawn, TargetError> {
        Err(TargetError::unsupported("Worker spawn", self.kind()))
    }

    fn worker_by_name(&self) -> Result<WorkerByName, TargetError> {
        Err(TargetError::unsupported(
            "Worker name attachment",
            self.kind(),
        ))
    }

    fn resume_worker(&self) -> Result<WorkerResume, TargetError> {
        Err(TargetError::unsupported("Worker resume", self.kind()))
    }

    fn dashboard(&self) -> Result<Dashboard, TargetError> {
        match self.resolve()? {
            ResolvedTarget::Backend {
                base_url,
                workspace_id,
            } => Ok(Dashboard::Backend {
                base_url,
                workspace_id,
            }),
            ResolvedTarget::Local | ResolvedTarget::Standalone => {
                unreachable!("BackendTarget cannot resolve as a local target")
            }
        }
    }

    fn list_workers(&self, request: WorkerListRequest) -> Result<WorkerList, TargetError> {
        Ok(WorkerList {
            local_runtime_command: None,
            backend_target: Some(BackendRuntimeListTarget::new(
                self.base_url.clone(),
                self.workspace_id.clone(),
                request.runtime_id,
            )),
            include_stopped: request.include_stopped,
        })
    }

    fn connect_worker(
        &self,
        selector: WorkerConnectionSelector,
    ) -> Result<WorkerConnection, TargetError> {
        let workspace_id = self.workspace_id.clone().ok_or_else(|| {
            TargetError::invalid(
                self.kind(),
                "workspace selection is required before connecting to a Backend Worker",
            )
        })?;
        Ok(WorkerConnection {
            target: BackendRuntimeTarget::new(
                self.base_url.clone(),
                workspace_id,
                selector.runtime_id,
                selector.worker_id,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_target_resolves_workspace_scoped_product_state_authority() {
        let target = BackendTarget::new("http://127.0.0.1:8787", Some("workspace-a"));

        assert_eq!(
            target.resolve().unwrap(),
            ResolvedTarget::Backend {
                base_url: "http://127.0.0.1:8787".to_string(),
                workspace_id: "workspace-a".to_string(),
            }
        );
    }

    #[test]
    fn backend_target_rejects_product_state_resolution_without_workspace() {
        let target = BackendTarget::new("http://127.0.0.1:8787", None::<String>);

        assert!(
            target
                .resolve()
                .unwrap_err()
                .to_string()
                .contains("workspace selection is required")
        );
    }

    #[test]
    fn local_target_resolves_local_product_state_authority() {
        assert_eq!(LocalTarget::new().resolve().unwrap(), ResolvedTarget::Local);
    }

    #[test]
    fn standalone_target_carries_in_process_state_without_resolving_runtime_command() {
        let target = StandaloneTarget::new("/tmp/yoi-standalone-state");

        assert_eq!(target.kind(), TargetKind::Standalone);
        assert_eq!(target.resolve().unwrap(), ResolvedTarget::Standalone);
        assert_eq!(
            target.spawn_worker().unwrap(),
            WorkerSpawn::Standalone {
                state_dir: PathBuf::from("/tmp/yoi-standalone-state"),
            }
        );
    }

    #[test]
    fn standalone_target_never_falls_back_to_legacy_local_operations() {
        let target = StandaloneTarget::new("/tmp/yoi-standalone-state");

        assert_eq!(
            target.worker_by_name().unwrap_err().to_string(),
            "Worker name attachment is not supported by Standalone target"
        );
        assert_eq!(
            target
                .list_workers(WorkerListRequest::new(None))
                .unwrap_err()
                .to_string(),
            "Worker listing is not supported by Standalone target"
        );
    }

    #[test]
    fn backend_target_builds_workspace_scoped_dashboard() {
        let target = BackendTarget::new("http://127.0.0.1:8787", Some("workspace-a"));

        assert_eq!(
            target.dashboard().unwrap(),
            Dashboard::Backend {
                base_url: "http://127.0.0.1:8787".to_string(),
                workspace_id: "workspace-a".to_string(),
            }
        );
    }

    #[test]
    fn backend_target_rejects_dashboard_without_workspace_selection() {
        let target = BackendTarget::new("http://127.0.0.1:8787", None::<String>);

        assert!(
            target
                .dashboard()
                .unwrap_err()
                .to_string()
                .contains("workspace selection is required")
        );
    }

    #[test]
    fn backend_target_builds_worker_list() {
        let target = BackendTarget::new("http://127.0.0.1:8787", Some("workspace-a"));
        let workers = target
            .list_workers(WorkerListRequest::new(Some("runtime-a".to_string())))
            .unwrap();

        assert_eq!(
            workers.backend_target.as_ref().unwrap().base_url,
            "http://127.0.0.1:8787"
        );
        assert_eq!(
            workers
                .backend_target
                .as_ref()
                .unwrap()
                .workspace_id
                .as_deref(),
            Some("workspace-a")
        );
        assert_eq!(
            workers
                .backend_target
                .as_ref()
                .unwrap()
                .runtime_id
                .as_deref(),
            Some("runtime-a")
        );
    }

    #[test]
    fn backend_target_builds_worker_connection() {
        let target = BackendTarget::new("http://127.0.0.1:8787", Some("workspace-a"));
        let connection = target
            .connect_worker(WorkerConnectionSelector::new("runtime-a", "worker-b"))
            .unwrap();

        assert_eq!(connection.target.base_url, "http://127.0.0.1:8787");
        assert_eq!(connection.target.workspace_id, "workspace-a");
        assert_eq!(connection.target.runtime_id, "runtime-a");
        assert_eq!(connection.target.worker_id, "worker-b");
    }

    #[test]
    fn backend_target_rejects_worker_connection_before_workspace_selection() {
        let target = BackendTarget::new("http://127.0.0.1:8787", None::<String>);
        let error =
            match target.connect_worker(WorkerConnectionSelector::new("runtime-a", "worker-b")) {
                Ok(_) => panic!("unscoped connection must fail"),
                Err(error) => error,
            };

        assert!(
            error
                .to_string()
                .contains("workspace selection is required")
        );
    }

    #[test]
    fn backend_target_rejects_local_worker_operations() {
        let target = BackendTarget::new("http://127.0.0.1:8787", None::<String>);
        let err = target.spawn_worker().unwrap_err();

        assert_eq!(
            err.to_string(),
            "Worker spawn is not supported by Backend target"
        );
    }

    #[test]
    fn standalone_target_builds_explicit_session_intents() {
        let target = StandaloneTarget::new("/tmp/yoi-client-sessions");
        let list = target.standalone_session_list(true).unwrap();
        assert_eq!(list.state_dir, PathBuf::from("/tmp/yoi-client-sessions"));
        assert!(list.include_all);
        assert!(list.cwd.is_absolute());

        let resume = target
            .standalone_session_resume("019d1234-0000-7000-8000-000000000000".to_string())
            .unwrap();
        assert_eq!(resume.state_dir, list.state_dir);
        assert_eq!(resume.session_id, "019d1234-0000-7000-8000-000000000000");

        assert!(
            LocalTarget::new()
                .standalone_session_list(false)
                .unwrap_err()
                .to_string()
                .contains("not supported")
        );
    }

    #[test]
    fn local_target_builds_local_worker_list() {
        let target = LocalTarget::new();
        let workers = target
            .list_workers(WorkerListRequest::with_stopped(None))
            .unwrap();

        assert!(workers.local_runtime_command.is_some());
        assert!(workers.backend_target.is_none());
        assert!(workers.include_stopped);
    }
}
