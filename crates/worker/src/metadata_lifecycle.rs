//! Atomic metadata transitions with Host-owned Session lifecycle participants.
//!
//! Participants inspect domain evidence inside the metadata update, not after the
//! active pointer has been published. The generic writer never interprets it.

use session_store::worker_metadata::{
    WorkerActiveSegmentRef, WorkerMetadata, WorkerMetadataStore, WorkerStoreError,
};
use std::sync::Arc;

pub(crate) trait SessionMetadataLifecycle: Send + Sync {
    /// Runs under the metadata update lock. Only compare/transform the supplied
    /// value: no I/O, reentrant store calls, lock waits, or Feature refresh.
    fn active_changing(&self, metadata: &mut WorkerMetadata, next: Option<&WorkerActiveSegmentRef>);
}

pub(crate) type WorkerMetadataWriter =
    Arc<dyn Fn(WorkerMetadata) -> Result<(), WorkerStoreError> + Send + Sync>;
pub(crate) type WorkerMetadataSegmentCas = Arc<
    dyn Fn(&str, &WorkerActiveSegmentRef, WorkerActiveSegmentRef) -> Result<bool, WorkerStoreError>
        + Send
        + Sync,
>;

pub(crate) fn worker_metadata_writer_for_store<St>(store: &St) -> WorkerMetadataWriter
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    let store = store.clone();
    let participants = crate::feature::builtin::session_metadata_lifecycles();
    Arc::new(move |next| {
        store
            .update_by_name(&next.worker_name, |metadata| {
                for participant in &participants {
                    participant.active_changing(metadata, next.active.as_ref());
                }
                metadata.active = next.active;
                metadata.resolved_manifest_snapshot = next.resolved_manifest_snapshot;
                if next.workspace_id.is_some() {
                    metadata.workspace_id = next.workspace_id;
                }
                if next.workspace_root.is_some() {
                    metadata.workspace_root = next.workspace_root;
                }
            })
            .map(|_| ())
    })
}

pub(crate) fn worker_metadata_segment_cas_for_store<St>(store: &St) -> WorkerMetadataSegmentCas
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    let store = store.clone();
    let participants = crate::feature::builtin::session_metadata_lifecycles();
    Arc::new(move |worker_name, expected, replacement| {
        let mut matched = false;
        store.update_by_name(worker_name, |metadata| {
            if metadata.active.as_ref() == Some(expected) {
                for participant in &participants {
                    participant.active_changing(metadata, Some(&replacement));
                }
                metadata.active = Some(replacement);
                matched = true;
            }
        })?;
        Ok(matched)
    })
}
