//! Reciprocal peer metadata registration, independent of discovery and transport.
//!
//! Registering a peer does not start a Worker or grant filesystem, observation, or
//! messaging authority. The two metadata writes are not a transaction: a failed
//! reciprocal write triggers best-effort restoration of the source's prior peers.

use serde::Serialize;
use session_store::{WorkerMetadataStore, WorkerStoreError, validate_worker_name};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct PeerRegistrationResult {
    pub source: String,
    pub peer: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PeerRegistrationError {
    #[error("worker state missing for `{worker_name}`")]
    StateMissing { worker_name: String },
    #[error("worker `{worker_name}` cannot be registered as a peer of itself")]
    SelfPeer { worker_name: String },
    #[error("worker `{worker_name}` does not exist")]
    MissingWorker { worker_name: String },
    #[error("worker store error: {0}")]
    WorkerStore(#[from] WorkerStoreError),
}

pub(crate) fn register_peer(
    store: &impl WorkerMetadataStore,
    source: &str,
    peer: &str,
) -> Result<PeerRegistrationResult, PeerRegistrationError> {
    ensure_existing_peer(store, source, peer)?.ok_or_else(|| PeerRegistrationError::MissingWorker {
        worker_name: peer.to_string(),
    })
}

fn ensure_existing_peer(
    store: &impl WorkerMetadataStore,
    source: &str,
    peer: &str,
) -> Result<Option<PeerRegistrationResult>, PeerRegistrationError> {
    validate_worker_name(peer)?;
    if peer == source {
        return Err(PeerRegistrationError::SelfPeer {
            worker_name: peer.to_string(),
        });
    }
    let prior_source_peers = store
        .read_by_name(source)?
        .ok_or_else(|| PeerRegistrationError::StateMissing {
            worker_name: source.to_string(),
        })?
        .peers;
    if store.read_by_name(peer)?.is_none() {
        return Ok(None);
    }

    store.add_peer(source, peer)?;
    if let Err(error) = store.add_peer(peer, source) {
        let _ = store.set_peers(source, prior_source_peers);
        return Err(PeerRegistrationError::WorkerStore(error));
    }
    Ok(Some(PeerRegistrationResult {
        source: source.to_string(),
        peer: peer.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use session_store::{FsWorkerStore, WorkerMetadata};

    #[test]
    fn register_peer_persists_reciprocal_metadata_without_duplicates() {
        let root = tempfile::tempdir().unwrap();
        let store = FsWorkerStore::new(root.path()).unwrap();
        store.write(&WorkerMetadata::new("source", None)).unwrap();
        store.write(&WorkerMetadata::new("target", None)).unwrap();
        for _ in 0..2 {
            let result = register_peer(&store, "source", "target").unwrap();
            assert_eq!(
                serde_json::to_value(result).unwrap(),
                serde_json::json!({
                    "source": "source", "peer": "target",
                })
            );
        }
        // Reopen the real store rather than asserting only the write calls.
        let persisted = FsWorkerStore::new(root.path()).unwrap();
        let source = persisted.read_by_name("source").unwrap().unwrap();
        let target = persisted.read_by_name("target").unwrap().unwrap();
        assert_eq!(source.peers.len(), 1);
        assert_eq!(source.peers[0].worker_name, "target");
        assert_eq!(target.peers.len(), 1);
        assert_eq!(target.peers[0].worker_name, "source");
    }

    #[test]
    fn invalid_self_or_missing_peer_leaves_metadata_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let store = FsWorkerStore::new(root.path()).unwrap();
        store.write(&WorkerMetadata::new("source", None)).unwrap();
        assert!(matches!(
            register_peer(&store, "source", "source"),
            Err(PeerRegistrationError::SelfPeer { .. })
        ));
        assert!(matches!(
            register_peer(&store, "source", "missing"),
            Err(PeerRegistrationError::MissingWorker { .. })
        ));
        assert!(matches!(
            register_peer(&store, "source", "../escape"),
            Err(PeerRegistrationError::WorkerStore(_))
        ));
        assert!(matches!(
            register_peer(&store, "missing-source", "source"),
            Err(PeerRegistrationError::StateMissing { .. })
        ));
        assert!(
            store
                .read_by_name("source")
                .unwrap()
                .unwrap()
                .peers
                .is_empty()
        );
        assert_eq!(store.list_names().unwrap(), vec!["source"]);
    }

    struct FailTargetPeerStore(FsWorkerStore);

    impl WorkerMetadataStore for FailTargetPeerStore {
        fn write(&self, metadata: &WorkerMetadata) -> Result<(), WorkerStoreError> {
            if metadata.worker_name == "target" && !metadata.peers.is_empty() {
                return Err(WorkerStoreError::Io(std::io::Error::other(
                    "target write failed",
                )));
            }
            self.0.write(metadata)
        }

        fn read_by_name(&self, name: &str) -> Result<Option<WorkerMetadata>, WorkerStoreError> {
            self.0.read_by_name(name)
        }

        fn list_names(&self) -> Result<Vec<String>, WorkerStoreError> {
            self.0.list_names()
        }

        fn delete_by_name(&self, name: &str) -> Result<(), WorkerStoreError> {
            self.0.delete_by_name(name)
        }
    }

    #[test]
    fn reciprocal_write_failure_restores_exact_prior_source_peers() {
        let root = tempfile::tempdir().unwrap();
        let inner = FsWorkerStore::new(root.path()).unwrap();
        for name in ["source", "target", "existing"] {
            inner.write(&WorkerMetadata::new(name, None)).unwrap();
        }
        inner.add_peer("source", "existing").unwrap();
        for had_target in [false, true] {
            if had_target {
                inner.add_peer("source", "target").unwrap();
            }
            let before = inner.read_by_name("source").unwrap().unwrap().peers;
            let store = FailTargetPeerStore(inner.clone());
            assert!(matches!(
                register_peer(&store, "source", "target"),
                Err(PeerRegistrationError::WorkerStore(_))
            ));
            assert_eq!(inner.read_by_name("source").unwrap().unwrap().peers, before);
            assert!(
                inner
                    .read_by_name("target")
                    .unwrap()
                    .unwrap()
                    .peers
                    .is_empty()
            );
        }
    }
}
