use std::fmt;

use crate::{BackendRuntimeListTarget, BackendRuntimeTarget, WorkerRuntimeCommand};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Local,
    Backend,
}

impl fmt::Display for TargetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => f.write_str("local"),
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
pub struct WorkerSpawn {
    pub runtime_command: WorkerRuntimeCommand,
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
pub struct Dashboard {
    pub runtime_command: WorkerRuntimeCommand,
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

    fn spawn_worker(&self) -> Result<WorkerSpawn, TargetError>;

    fn worker_by_name(&self) -> Result<WorkerByName, TargetError>;

    fn resume_worker(&self) -> Result<WorkerResume, TargetError>;

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

    fn spawn_worker(&self) -> Result<WorkerSpawn, TargetError> {
        Ok(WorkerSpawn {
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
        Ok(Dashboard {
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

impl Target for BackendTarget {
    fn kind(&self) -> TargetKind {
        TargetKind::Backend
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
        Err(TargetError::unsupported("Dashboard", self.kind()))
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
        Ok(WorkerConnection {
            target: BackendRuntimeTarget::new(
                self.base_url.clone(),
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
        assert_eq!(connection.target.runtime_id, "runtime-a");
        assert_eq!(connection.target.worker_id, "worker-b");
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
