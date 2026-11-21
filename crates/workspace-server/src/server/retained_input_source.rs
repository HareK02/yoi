//! A read view of the original lifecycle intent, not a second writer owner.
//! Release keeps live pinned originals; removal supplies the same scope from an archive.
use super::*;
use crate::store::{
    SubjectReleaseInputHandoffProgress, WorkerInputHandoffBinding, WorkerInputHandoffMember,
    WorkerInputHandoffProgress,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RetainedSourceOrigin {
    Removal {
        operation_id: String,
        input_fingerprint: String,
        archive_id: String,
    },
    Release {
        operation_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::server) struct RetainedSourceProgress {
    pub worker: RuntimeWorkerRef,
    pub binding: WorkerInputHandoffBinding,
    pub members: Vec<WorkerInputHandoffMember>,
    origin: RetainedSourceOrigin,
}

impl RetainedSourceProgress {
    pub(super) fn removal(progress: &WorkerInputHandoffProgress) -> Result<Self> {
        Ok(Self {
            worker: progress.worker.clone(),
            binding: progress.metadata.binding.clone(),
            members: progress
                .metadata
                .members
                .clone()
                .ok_or_else(|| Error::Store("Retained scope absent".into()))?,
            origin: RetainedSourceOrigin::Removal {
                operation_id: progress.operation_id.clone(),
                input_fingerprint: progress.input_fingerprint.clone(),
                archive_id: progress
                    .metadata
                    .released_input_handoff
                    .as_ref()
                    .ok_or_else(|| Error::Store("Archive handoff absent".into()))?
                    .archive_id
                    .clone(),
            },
        })
    }

    pub(super) fn release(progress: &SubjectReleaseInputHandoffProgress) -> Self {
        Self {
            worker: progress.worker.clone(),
            binding: progress.handoff.binding.clone(),
            members: progress.handoff.members.clone(),
            origin: RetainedSourceOrigin::Release {
                operation_id: progress.operation_id.clone(),
            },
        }
    }

    pub(super) fn material_source(&self) -> Result<runtime_api::SessionPublicSource> {
        Ok(match &self.origin {
            RetainedSourceOrigin::Removal { archive_id, .. } => {
                runtime_api::SessionPublicSource::Archived {
                    archive_id: archive_id.clone(),
                }
            }
            RetainedSourceOrigin::Release { .. } => runtime_api::SessionPublicSource::Retained {
                worker_id: self
                    .worker
                    .worker_id
                    .parse()
                    .map_err(|_| Error::Store("Invalid retained Worker identity".into()))?,
            },
        })
    }

    pub(super) fn capture(
        &self,
        api: &WorkspaceApi,
        member: &runtime_api::WorkerRemovalSessionMember,
        order: u64,
        offset: u64,
        budget: usize,
    ) -> Result<session_store::capture::CaptureDto> {
        let start = session_store::capture::RecordingPosition {
            canonical_order: order,
            byte_offset: offset,
        };
        let slice = match &self.origin {
            RetainedSourceOrigin::Removal { operation_id, .. } => {
                api.runtime.capture_worker_removal_session(
                    &self.worker,
                    runtime_api::WorkerRemovalSessionCaptureRequest {
                        workspace_id: api.workspace_id().into(),
                        operation_id: operation_id.clone(),
                        session_id: member.session_id,
                        active_segment_id: member.active_segment_id,
                        start,
                        max_entries: MAX_JOURNAL_RANGES,
                        max_serialized_bytes: budget,
                    },
                )
            }
            RetainedSourceOrigin::Release { operation_id } => {
                api.runtime.capture_subject_release_session(
                    &self.worker,
                    runtime_api::SubjectReleaseSessionCaptureRequest {
                        workspace_id: api.workspace_id().into(),
                        operation_id: operation_id.clone(),
                        binding_id: self.binding.writer_binding_id.clone(),
                        session_id: member.session_id,
                        active_segment_id: member.active_segment_id,
                        start,
                        max_entries: MAX_JOURNAL_RANGES,
                        max_serialized_bytes: budget,
                    },
                )
            }
        }
        .map_err(RuntimeRegistryError::into_error)?;
        removal::validate_capture(member, slice, budget)
    }
}

pub(super) fn prepare_release(
    api: &WorkspaceApi,
    store: &crate::subjektiv::SubjektivStore,
    identity: &feature_storage::DatabaseIdentity,
    lease: &crate::store::WorkerSingletonLease,
    binding: &JournalWriterBinding,
    operation: &str,
    scope: &runtime_api::SubjectReleaseRecordingScope,
) -> Result<()> {
    removal::verify_members(&scope.members)?;
    let mut sessions = store
        .subject_session_attributions(&binding.subject_id)
        .map_err(subjektiv_store_error)?
        .into_iter()
        .filter(|attr| attr.runtime_id == binding.runtime_id && attr.worker_id == binding.worker_id)
        .collect::<Vec<_>>();
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    let attributed = sessions
        .iter()
        .map(|attr| attr.session_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let frozen = scope
        .members
        .iter()
        .map(|member| member.session_id.to_string())
        .collect::<std::collections::BTreeSet<_>>();
    if attributed != frozen || sessions.is_empty() {
        return Err(Error::RepositoryConflict(
            "Release recording attribution differs from fixed scope".into(),
        ));
    }
    let metadata = crate::store::SubjectReleaseInputHandoff {
        binding: WorkerInputHandoffBinding {
            subject_id: binding.subject_id.clone(),
            writer_session_id: sessions[0].session_id.clone(),
            writer_binding_id: binding.binding_id.clone(),
            database: identity.clone(),
        },
        members: scope
            .members
            .iter()
            .map(WorkerInputHandoffMember::from)
            .collect(),
        legacy_completed: false,
        handoff_status: crate::store::InputHandoffStatus::Pending,
        cause: None,
    };
    let prepared = SubjectReleaseInputHandoffProgress {
        workspace_id: api.workspace_id().into(),
        worker: lease.worker.clone(),
        operation_id: operation.into(),
        released: false,
        handoff: metadata,
    };
    let source = RetainedSourceProgress::release(&prepared);
    for member in &scope.members {
        let attr = sessions
            .iter()
            .find(|attr| attr.session_id == member.session_id.to_string())
            .expect("set equality");
        let host = JournalHostContext::validated_session(store, attr.clone()).map_err(domain)?;
        if store.journal_session_erased(&host).map_err(domain)? {
            return Err(Error::RepositoryConflict(
                "Session erased during release handoff".into(),
            ));
        }
        if !member.materialized {
            continue;
        }
        retain_source(
            api.runtime.as_ref(),
            api.workspace_id(),
            attr,
            source.material_source()?,
            false,
        )?;
        // The union reader validates the entire fixed catalog, including detached
        // originals, before returning even the first bounded slice.
        source.capture(api, member, 0, 0, JournalInput::capture_budget())?;
    }
    api.config_store.prepare_subject_release_input_handoff(
        api.workspace_id(),
        lease,
        operation,
        &prepared.handoff.binding,
        &prepared.handoff.members,
    )?;
    Ok(())
}

pub(super) fn recover_release_input_handoffs(api: &WorkspaceApi) -> Result<()> {
    let mut after = None;
    loop {
        let page = api
            .config_store
            .list_pending_subject_release_input_handoffs(
                api.workspace_id(),
                after.as_deref(),
                100,
            )?;
        for progress in &page {
            let outcome = (|| {
                // A subsequent ordinary removal owns transfer and unpinning. Do not
                // race its archive publication or revive a second recording drain.
                if let Some(prepared) = api
                    .config_store
                    .recover_worker_removal_plan(api.workspace_id(), &progress.worker)
                    .map_err(|error| Error::Store(error.to_string()))?
                    && let Some(removal) = api.store.get_worker_input_handoff_for_plan(&prepared)?
                    && let Some(recording) = &removal.metadata.released_input_handoff
                {
                    if removal.metadata.binding.subject_id != progress.handoff.binding.subject_id
                        || removal.metadata.binding.database != progress.handoff.binding.database
                        || removal.metadata.binding.writer_binding_id
                            != progress.handoff.binding.writer_binding_id
                        || removal.metadata.members.as_ref() != Some(&progress.handoff.members)
                    {
                        return Err(Error::RepositoryConflict(
                            "Removal changed released recording scope".into(),
                        ));
                    }
                    return Ok(recording.handoff_status);
                }
                removal::admit_retained_input(api, &RetainedSourceProgress::release(progress))
            })();
            let (complete, cause) = match outcome {
                Ok(status) => (status, None),
                Err(error) => {
                    tracing::warn!(operation_id=%progress.operation_id, %error, "Released Journal recording awaits recovery");
                    let cause = match error {
                        Error::WorkspacePermissionDenied(_) => {
                            crate::store::WorkerInputHandoffCause::OwnershipUnknown
                        }
                        Error::SubjektivStorage(_) => {
                            crate::store::WorkerInputHandoffCause::StorageUnavailable
                        }
                        _ => crate::store::WorkerInputHandoffCause::SourceUnavailable,
                    };
                    (crate::store::InputHandoffStatus::Pending, Some(cause))
                }
            };
            match api
                .config_store
                .update_subject_release_input_handoff(progress, complete, cause)
            {
                Err(error) => {
                    tracing::debug!(operation_id=%progress.operation_id, %error, "Release input handoff changed")
                }
                Ok(()) if complete == crate::store::InputHandoffStatus::Admitted => {
                    if let Err(error) = removal::release_original_pins(
                        api,
                        &RetainedSourceProgress::release(progress),
                    ) {
                        tracing::warn!(operation_id=%progress.operation_id, %error, "Job owns input; original pin cleanup remains unconfirmed");
                    }
                }
                Ok(()) => {}
            }
        }
        if page.len() < 100 {
            return Ok(());
        }
        after = page.last().map(|p| p.operation_id.clone());
    }
}
