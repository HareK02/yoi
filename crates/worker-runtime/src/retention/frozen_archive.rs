//! Publish a verified copy before transferring recording responsibility. This is
//! not a removal receipt: the source, its ingress fence and all Session pins stay
//! owned by their existing contracts until Backend completes the handoff.

use super::*;

impl FsWorkerRetentionProvider {
    pub(crate) fn publish_frozen_archive(
        &self,
        request: &WorkerRetentionExecutionRequest,
        freeze: &WorkerRemovalFreeze,
    ) -> Result<WorkerSessionArchiveManifest, RuntimeError> {
        validate_request(request)?;
        if request.session_disposition != SessionDisposition::Archive
            || freeze.operation_id != request.operation_id
            || freeze.input_fingerprint != request.input_fingerprint
            || freeze.workspace_id != request.workspace_id
            || freeze.source_runtime_id != request.source_runtime_id
            || freeze.worker_id != request.worker_id
            || freeze.members.is_empty()
        {
            return Err(RuntimeError::InvalidRequest(
                "Frozen archive publication identity mismatch".into(),
            ));
        }
        let archive_id = request.archive_id.as_deref().ok_or_else(|| {
            RuntimeError::InvalidRequest("Frozen archive identity is missing".into())
        })?;
        let retention = runtime_session_retention(&self.runtime_root);
        let mut sessions = freeze
            .members
            .iter()
            .map(|m| m.session_id)
            .collect::<Vec<_>>();
        sessions.sort();
        sessions.dedup();
        if sessions.len() != freeze.members.len() {
            return Err(RuntimeError::InvalidRequest(
                "Duplicate frozen Session".into(),
            ));
        }
        // Keep the same live/erasure fences and pins on both copies. Publication
        // neither requires Journal completion nor releases its existing pins.
        let guards = sessions
            .iter()
            .map(|session| retention.lock_live(*session))
            .collect::<Result<Vec<_>, _>>()
            .map_err(retention_error)?;
        let worker_dir = self.worker_dir(request.worker_id);
        let manifest = if worker_dir.is_dir() {
            commit_session_archive(self, request, &worker_dir)?
        } else {
            // Exact replay after source removal reads the already published copy;
            // absence of the source is never interpreted as an empty recording.
            let (manifest, _) = self.session_archive_identity(archive_id)?;
            validate_existing_archive(&self.archive_dir(archive_id)?, &manifest, false)?
        };
        if manifest.archive_id != archive_id
            || manifest.workspace_id != request.workspace_id
            || manifest.source_runtime_id != request.source_runtime_id
            || manifest.source_worker_id != request.worker_id
            || manifest.operation_id != request.operation_id
            || manifest.input_fingerprint != request.input_fingerprint
            || manifest.source_created_at != request.source_created_at
            || manifest.removed_at != request.removed_at
            || manifest.effective_profile != request.effective_profile
            || manifest.retention_class != request.retention_class
            || manifest.policy_id != request.policy_id
        {
            return Err(RuntimeError::InvalidRequest(
                "Frozen archive receipt mismatch".into(),
            ));
        }
        let archive_root = self.archive_dir(archive_id)?.join("session");
        if freeze.members.len() != manifest.retained_sessions.len() + 1 {
            return Err(RuntimeError::InvalidRequest(
                "Frozen archive Session set mismatch".into(),
            ));
        }
        for member in &freeze.members {
            let guard = &guards[sessions
                .binary_search(&member.session_id)
                .expect("locked member")];
            let primary = member.session_id.to_string() == manifest.source_session_id;
            let active = if primary {
                manifest.source_active_segment_id
            } else {
                manifest
                    .retained_sessions
                    .iter()
                    .find(|binding| binding.session_id == member.session_id)
                    .map(|binding| binding.active_segment_id)
            };
            if active != member.active_segment_id || (!primary && active.is_none()) {
                return Err(RuntimeError::InvalidRequest(
                    "Frozen archive lineage mismatch".into(),
                ));
            }
            let descriptors = match active {
                Some(segment) if primary => {
                    session_store::capture::recording_union_catalog_worker_guarded(
                        &archive_root,
                        member.session_id,
                        segment,
                        Default::default(),
                        guard,
                    )
                    .map_err(|error| {
                        RuntimeError::RecordingSourceUnavailable {
                            message: error.to_string(),
                        }
                    })?
                }
                Some(segment) => session_store::capture::recording_union_catalog_fs_guarded(
                    &archive_root.join("retained-sessions"),
                    member.session_id,
                    segment,
                    Default::default(),
                    guard,
                )
                .map_err(|error| RuntimeError::RecordingSourceUnavailable {
                    message: error.to_string(),
                })?,
                None if !member.materialized && member.descriptors.is_empty() => Vec::new(),
                None => {
                    return Err(RuntimeError::InvalidRequest(
                        "Frozen archive source is missing".into(),
                    ));
                }
            };
            if descriptors != member.descriptors
                || member.end.byte_offset != 0
                || member.end.canonical_order != descriptors.len() as u64
            {
                return Err(RuntimeError::InvalidRequest(
                    "Archived originals do not match the complete frozen recording scope".into(),
                ));
            }
        }
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Storage-only crash-window test. Runtime Stop/freeze authority is exercised
    // by worker_backend::removal_tests, not by this supplied provider fixture.
    #[test]
    fn frozen_archive_replay_preserves_attested_unmaterialized_session_but_rejects_missing_archive()
    {
        let root = tempfile::tempdir().unwrap();
        let worker = WorkerId::now_v7();
        let session = session_store::new_session_id();
        let request = super::super::tests::request(worker, SessionDisposition::Archive);
        let provider = FsWorkerRetentionProvider::new(root.path());
        let mut metadata = session_store::WorkerMetadata::new(
            worker.to_string(),
            Some(session_store::WorkerActiveSegmentRef::pending_segment(
                session,
            )),
        );
        metadata.workspace_id = Some(request.workspace_id.clone());
        super::super::tests::write_json(
            &provider.worker_dir(worker).join("metadata.json"),
            &metadata,
        );
        let freeze = WorkerRemovalFreeze {
            operation_id: request.operation_id.clone(),
            input_fingerprint: request.input_fingerprint.clone(),
            workspace_id: request.workspace_id.clone(),
            source_runtime_id: request.source_runtime_id.clone(),
            worker_id: worker,
            members: vec![WorkerRemovalSessionMember {
                session_id: session,
                active_segment_id: None,
                materialized: false,
                end: Default::default(),
                descriptors: Vec::new(),
            }],
        };
        let manifest = provider.publish_frozen_archive(&request, &freeze).unwrap();
        assert_eq!(manifest.content_file_count, 0);
        assert!(provider.worker_dir(worker).exists());
        std::fs::remove_dir_all(provider.worker_dir(worker)).unwrap();
        assert_eq!(
            provider.publish_frozen_archive(&request, &freeze).unwrap(),
            manifest
        );
        std::fs::remove_dir_all(
            provider
                .archive_dir(request.archive_id.as_ref().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert!(provider.publish_frozen_archive(&request, &freeze).is_err());
    }
}
