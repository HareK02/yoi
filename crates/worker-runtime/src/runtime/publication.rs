use super::*;
use crate::catalog::{
    RepositoryPublicationBinding, RepositoryPublicationReceipt, RepositoryPublicationRequest,
};

impl Runtime {
    pub async fn publish_repository_source_from_resource(
        &self,
        mut request: RepositoryPublicationRequest,
    ) -> Result<RepositoryPublicationReceipt, RuntimeError> {
        let backend = self.publication_backend(&request)?;
        if let Some(saved) = backend.repository_publication_receipt(&request.binding)? {
            return Ok(saved);
        }
        if let Some(ssh) = request.materialization.ssh.as_mut() {
            if self.resolve_repository_access_resource(ssh).await.is_err() {
                return backend
                    .reject_repository_publication(
                        &request,
                        "Current Repository credentials could not be resolved",
                    )
                    .map_err(RuntimeError::from);
            }
        }
        let runtime = self.clone();
        tokio::task::spawn_blocking(move || runtime.publish_repository_source(request))
            .await
            .map_err(|_| {
                RuntimeError::InvalidRequest(
                    "publication task outcome unavailable; reconcile operation".into(),
                )
            })?
    }

    pub async fn reconcile_repository_publication_from_resource(
        &self,
        mut request: RepositoryPublicationRequest,
    ) -> Result<RepositoryPublicationReceipt, RuntimeError> {
        // Recovery is authorized by the saved operation, not the former Worker.
        let backend = self.publication_backend(&request)?;
        let saved = backend
            .repository_publication_receipt(&request.binding)?
            .ok_or_else(|| {
                RuntimeError::InvalidRequest("publication operation was not found".into())
            })?;
        if !matches!(
            saved.outcome,
            crate::catalog::RepositoryPublicationOutcome::Unknown {}
        ) {
            return Ok(saved);
        }
        if let Some(ssh) = request.materialization.ssh.as_mut() {
            if self.resolve_repository_access_resource(ssh).await.is_err() {
                return Ok(saved);
            }
        }
        let runtime = self.clone();
        tokio::task::spawn_blocking(move || runtime.reconcile_repository_publication(request))
            .await
            .map_err(|_| {
                RuntimeError::InvalidRequest("publication recovery task unavailable".into())
            })?
    }

    /// Read retained provider evidence without current Repository configuration,
    /// credentials, Workdir access, or the former Worker's live authority.
    pub fn repository_publication_receipt(
        &self,
        binding: RepositoryPublicationBinding,
    ) -> Result<Option<RepositoryPublicationReceipt>, RuntimeError> {
        self.publication_binding_backend(&binding)?
            .repository_publication_receipt(&binding)
            .map_err(RuntimeError::from)
    }

    fn publication_binding_backend(
        &self,
        binding: &RepositoryPublicationBinding,
    ) -> Result<crate::execution::WorkerExecutionBackendRef, RuntimeError> {
        let state = self.lock()?;
        state.ensure_running()?;
        if state.runtime_identity.as_deref() != Some(binding.worker.runtime_id.as_str())
            || binding.workspace_id.is_empty()
            || binding.operation_id.is_empty()
        {
            return Err(RuntimeError::InvalidRequest(
                "publication Runtime or operation binding mismatch".into(),
            ));
        }
        state
            .execution_backend
            .clone()
            .ok_or_else(|| RuntimeError::ExecutionBackendUnavailable {
                message: "Repository publication requires an execution backend".into(),
            })
    }

    fn publication_backend(
        &self,
        request: &RepositoryPublicationRequest,
    ) -> Result<crate::execution::WorkerExecutionBackendRef, RuntimeError> {
        if request.materialization.runtime_id != request.binding.worker.runtime_id
            || request.materialization.workspace_id != request.binding.workspace_id
            || request.materialization.operation_id != request.binding.operation_id
            || request.repository.id != request.binding.repository_id
            || request.repository.source_fingerprint
                != request.binding.repository_source_fingerprint
        {
            return Err(RuntimeError::InvalidRequest(
                "publication Runtime or operation authority mismatch".into(),
            ));
        }
        self.publication_binding_backend(&request.binding)
    }

    pub fn publish_repository_source(
        &self,
        request: RepositoryPublicationRequest,
    ) -> Result<RepositoryPublicationReceipt, RuntimeError> {
        let backend = self.publication_backend(&request)?;
        if let Some(saved) = backend.repository_publication_receipt(&request.binding)? {
            return Ok(saved);
        }
        let publish = || -> Result<RepositoryPublicationReceipt, RuntimeError> {
            let worker = WorkerRef::try_from(&request.binding.worker).map_err(|_| {
                RuntimeError::InvalidRequest("publication Worker identity invalid".into())
            })?;
            // This is the same lifecycle/attachment gate used by Stop and attachment
            // replacement, not an unrelated publication-only mutex.
            let operation = self.worker_operation_lock(worker.worker_id)?;
            let _guard = operation.try_lock().map_err(|_| {
                RuntimeError::InvalidRequest("publication Worker operation is busy".into())
            })?;
            let _workdir =
                self.reserve_workdir_use([request.binding.working_directory_id.as_str()])?;
            {
                let state = self.lock()?;
                let record = state.worker(&worker)?;
                if !record.belongs_to_workspace(&request.binding.workspace_id)
                    || record.pending_lifecycle_operation_name().is_some()
                {
                    return Err(RuntimeError::InvalidRequest(
                        "publication Worker Workspace or lifecycle mismatch".into(),
                    ));
                }
                let attachment = record
                    .logical_workdir_attachments
                    .iter()
                    .find(|a| {
                        a.alias == request.binding.alias
                            && a.working_directory_id == request.binding.working_directory_id
                    })
                    .ok_or_else(|| {
                        RuntimeError::InvalidRequest("publication attachment mismatch".into())
                    })?;
                if !attachment
                    .capabilities
                    .supports(workdir::WorkdirSessionCapability::Command)
                    || !attachment
                        .capabilities
                        .supports(workdir::WorkdirSessionCapability::Write)
                {
                    return Err(RuntimeError::InvalidRequest(
                        "publication requires attachment command and write capabilities".into(),
                    ));
                }
            }
            backend
                .publish_repository_source(&request)
                .map_err(RuntimeError::from)
        };
        match publish() {
            Ok(receipt) => Ok(receipt),
            Err(_) => backend
                .reject_repository_publication(
                    &request,
                    "Publication preflight or required Worker/Workdir resource unavailable",
                )
                .map_err(RuntimeError::from),
        }
    }

    pub fn reconcile_repository_publication(
        &self,
        request: RepositoryPublicationRequest,
    ) -> Result<RepositoryPublicationReceipt, RuntimeError> {
        self.publication_backend(&request)?
            .reconcile_repository_publication(&request)
            .map_err(RuntimeError::from)
    }
}
