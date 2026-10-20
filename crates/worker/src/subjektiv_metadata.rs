//! Host-owned attribution transitions. Metadata callbacks are synchronous, local
//! comparisons only: no network I/O, Feature lookup, or nested store access.

use crate::metadata_lifecycle::SessionMetadataLifecycle;
use session_store::{
    SessionId, SubjektivSessionAttributionState, WorkerActiveSegmentRef, WorkerMetadata,
    WorkerMetadataStore,
};

pub(crate) struct SessionAttributionMetadata;

impl SessionMetadataLifecycle for SessionAttributionMetadata {
    fn active_changing(
        &self,
        metadata: &mut WorkerMetadata,
        next: Option<&WorkerActiveSegmentRef>,
    ) {
        if metadata.active.as_ref().map(|active| active.session_id)
            != next.map(|active| active.session_id)
        {
            metadata.subjektiv_session_attribution = None;
        }
    }
}

pub(crate) fn compare_and_swap_attribution<St: WorkerMetadataStore>(
    store: &St,
    worker_name: &str,
    session_id: SessionId,
    expected: Option<&SubjektivSessionAttributionState>,
    replacement: SubjektivSessionAttributionState,
) -> Result<(), crate::WorkerError> {
    let mut matched = false;
    store.update_by_name(worker_name, |metadata| {
        if metadata.active.as_ref().map(|active| active.session_id) == Some(session_id)
            && metadata.subjektiv_session_attribution.as_ref() == expected
            && replacement.session_id() == session_id
        {
            metadata.subjektiv_session_attribution = Some(replacement);
            matched = true;
        }
    })?;
    if !matched {
        return Err(crate::WorkerError::SubjektivSessionAttribution {
            state: "attribution_conflict",
            message: "active Session or observed attribution changed during Host confirmation"
                .into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Connect the production metadata writer/CAS and Host lifecycle to a real
    //! filesystem store; no metadata or persistence boundary is substituted.
    use super::*;
    use crate::metadata_lifecycle::{
        worker_metadata_segment_cas_for_store, worker_metadata_writer_for_store,
    };

    #[test]
    fn host_transition_preserves_same_session_and_invalidates_new_session_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsWorkerStore::new(dir.path()).unwrap();
        let session = session_store::new_session_id();
        let mut metadata = WorkerMetadata::new(
            "body",
            Some(WorkerActiveSegmentRef::pending_segment(session)),
        );
        metadata.workspace_id = Some("workspace".into());
        metadata.subjektiv_session_attribution =
            Some(SubjektivSessionAttributionState::Confirmed {
                session_id: session,
                subject_id: "subject".into(),
            });
        store.write(&metadata).unwrap();
        let writer = worker_metadata_writer_for_store(&store);
        let cas = worker_metadata_segment_cas_for_store(&store);
        let segment =
            WorkerActiveSegmentRef::active_segment(session, session_store::new_segment_id());
        let mut next = WorkerMetadata::new("body", Some(segment.clone()));
        writer(next.clone()).unwrap();
        let materialized = store.read_by_name("body").unwrap().unwrap();
        assert_eq!(
            materialized.subjektiv_session_attribution,
            metadata.subjektiv_session_attribution
        );
        assert_eq!(materialized.workspace_id, metadata.workspace_id);
        let compacted =
            WorkerActiveSegmentRef::active_segment(session, session_store::new_segment_id());
        assert!(cas("body", &segment, compacted.clone()).unwrap());
        assert_eq!(
            store
                .read_by_name("body")
                .unwrap()
                .unwrap()
                .subjektiv_session_attribution,
            metadata.subjektiv_session_attribution
        );
        let new_session = WorkerActiveSegmentRef::pending_segment(session_store::new_session_id());
        assert!(!cas("body", &segment, new_session.clone()).unwrap());
        assert!(cas("body", &compacted, new_session.clone()).unwrap());
        let switched = store.read_by_name("body").unwrap().unwrap();
        assert_eq!(switched.active, Some(new_session));
        assert!(switched.subjektiv_session_attribution.is_none());
        assert_eq!(switched.workspace_id, metadata.workspace_id);
        // Returning to the old Session cannot recover the invalidated evidence.
        next.active = metadata.active;
        writer(next).unwrap();
        assert!(
            store
                .read_by_name("body")
                .unwrap()
                .unwrap()
                .subjektiv_session_attribution
                .is_none()
        );
    }
}
