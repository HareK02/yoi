//! Durable, domain-neutral Session retention pins and permanent erasure fences.
//!
//! Hosts choose a control root whose lifetime outlasts the physical Session.
//! Every deletion path must execute its physical removal inside `delete`, and
//! publishers must hold `lock_live` while creating or appending source data.
//! A check followed by unlocked removal is not a retention guarantee.
//!
//! Pins have explicit owners and no timer/expiry: domain code releases its pin
//! only after its durable consumer no longer needs originals. A permanent erase
//! persists its fence before physical removal, invalidates pins, and rejects
//! late pins/publication even after reopening the control store. Retrying an
//! interrupted physical erase uses the same intent and is safe.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};

use crate::{SessionId, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionRetentionPin(pub uuid::Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionDeletionIntent {
    Ordinary,
    ErasePermanently,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionState {
    erased: bool,
    pins: BTreeSet<SessionRetentionPin>,
}

#[derive(Debug, Clone)]
pub struct SessionRetentionStore {
    root: PathBuf,
}

/// A process-independent lock; dropping it releases the filesystem lock.
/// Do not recursively acquire the same Session lock inside a guarded callback.
pub struct SessionRetentionGuard {
    _file: File,
    session_id: SessionId,
}

impl SessionRetentionGuard {
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }
}

impl SessionRetentionStore {
    pub fn new(control_root: impl Into<PathBuf>) -> Self {
        Self {
            root: control_root.into(),
        }
    }

    fn state_path(&self, session: SessionId) -> PathBuf {
        self.root.join(format!("{session}.json"))
    }

    fn lock(&self, session: SessionId) -> Result<SessionRetentionGuard, StoreError> {
        fs::create_dir_all(&self.root)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Control roots are Host-owned; never follow a replaced lock file.
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(self.root.join(format!("{session}.lock")))?;
        FileExt::lock_exclusive(&file)?;
        Ok(SessionRetentionGuard {
            _file: file,
            session_id: session,
        })
    }

    fn read(&self, session: SessionId) -> Result<RetentionState, StoreError> {
        match fs::read(self.state_path(session)) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(RetentionState::default())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn save(&self, session: SessionId, state: &RetentionState) -> Result<(), StoreError> {
        crate::worker_session_store::atomic_write_bytes(
            &self.state_path(session),
            &serde_json::to_vec(state)?,
        )
    }

    /// Observational fence check; unlike `lock_live`, never creates storage.
    /// Hosts still fence result adoption in their own durable consumer: a read
    /// finishing before erasure is not authority for a delayed domain write.
    pub fn ensure_live(&self, session: SessionId) -> Result<(), StoreError> {
        if self.read(session)?.erased {
            return Err(StoreError::SessionErased(session));
        }
        Ok(())
    }

    pub fn lock_live(&self, session: SessionId) -> Result<SessionRetentionGuard, StoreError> {
        let guard = self.lock(session)?;
        if self.read(session)?.erased {
            return Err(StoreError::SessionErased(session));
        }
        Ok(guard)
    }

    /// Protect a Host-owned Session before its first source publication. This is
    /// retention intent, not proof that any original material exists. The owner must
    /// establish it before accepting input and use the same control store for writes
    /// and deletion. Source-serving APIs must still verify actual material separately.
    pub fn retain_before_publication(
        &self,
        session: SessionId,
        pin: SessionRetentionPin,
    ) -> Result<(), StoreError> {
        let _guard = self.lock_live(session)?;
        let mut state = self.read(session)?;
        if state.pins.insert(pin) {
            self.save(session, &state)?;
        }
        Ok(())
    }

    /// Call `source_exists` under the same lock used by deletion. Replaying the
    /// same pin is idempotent; missing source never becomes a successful lease.
    pub fn pin(
        &self,
        session: SessionId,
        pin: SessionRetentionPin,
        source_exists: impl FnOnce() -> Result<bool, StoreError>,
    ) -> Result<(), StoreError> {
        let _guard = self.lock_live(session)?;
        if !source_exists()? {
            return Err(StoreError::SessionSourceMissing(session));
        }
        let mut state = self.read(session)?;
        if state.pins.insert(pin) {
            self.save(session, &state)?;
        }
        Ok(())
    }

    pub fn release(&self, session: SessionId, pin: SessionRetentionPin) -> Result<(), StoreError> {
        let _guard = self.lock(session)?;
        let mut state = self.read(session)?;
        if state.pins.remove(&pin) {
            self.save(session, &state)?;
        }
        Ok(())
    }

    pub fn pins(&self, session: SessionId) -> Result<Vec<SessionRetentionPin>, StoreError> {
        let _guard = self.lock(session)?;
        Ok(self.read(session)?.pins.into_iter().collect())
    }

    /// Execute physical removal while fencing concurrent pins and publishers.
    /// An erasure fence is retained even if removal fails or the source is
    /// already absent. Ordinary deletion after erasure is rejected: recovery
    /// must preserve the caller's explicit erase intent.
    pub fn delete<T>(
        &self,
        session: SessionId,
        intent: SessionDeletionIntent,
        remove: impl FnOnce() -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let _guard = self.lock(session)?;
        let mut state = self.read(session)?;
        match intent {
            SessionDeletionIntent::Ordinary => {
                if state.erased {
                    return Err(StoreError::SessionErased(session));
                }
                if !state.pins.is_empty() {
                    return Err(StoreError::SessionPinned(session));
                }
            }
            SessionDeletionIntent::ErasePermanently => {
                if !state.erased || !state.pins.is_empty() {
                    state.erased = true;
                    state.pins.clear();
                    self.save(session, &state)?;
                }
            }
        }
        remove()
    }
}

/// FsStore's control state is outside all per-Session source directories.
pub(crate) fn fs_retention(root: &Path) -> SessionRetentionStore {
    SessionRetentionStore::new(root.join(".retention"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_intent_survives_reopen_without_attesting_missing_originals() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let controls = temp.path().join("controls");
        let session = crate::new_session_id();
        let pin = SessionRetentionPin(uuid::Uuid::now_v7());
        let retention = SessionRetentionStore::new(&controls);
        retention.retain_before_publication(session, pin).unwrap();
        let reopened = SessionRetentionStore::new(&controls);
        assert_eq!(reopened.pins(session).unwrap(), [pin]);
        // An early owner is not a successful source-availability receipt.
        assert!(
            matches!(reopened.pin(session, pin, || Ok(source.is_file())), Err(StoreError::SessionSourceMissing(id)) if id == session)
        );
        {
            let _publication = reopened.lock_live(session).unwrap();
            fs::write(&source, "committed before notification").unwrap();
        }
        assert!(
            matches!(reopened.delete(session, SessionDeletionIntent::Ordinary, || { fs::remove_file(&source)?; Ok(()) }), Err(StoreError::SessionPinned(id)) if id == session)
        );
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "committed before notification"
        );
        reopened
            .delete(session, SessionDeletionIntent::ErasePermanently, || {
                fs::remove_file(&source)?;
                Ok(())
            })
            .unwrap();
        assert!(
            matches!(retention.retain_before_publication(session, pin), Err(StoreError::SessionErased(id)) if id == session)
        );
    }

    #[test]
    fn pin_survives_reopen_blocks_delete_and_releases_idempotently() {
        let temp = tempfile::tempdir().unwrap();
        let id = crate::new_session_id();
        let pin = SessionRetentionPin(uuid::Uuid::now_v7());
        let store = SessionRetentionStore::new(temp.path());
        store.pin(id, pin, || Ok(true)).unwrap();
        store.pin(id, pin, || Ok(true)).unwrap();
        let reopened = SessionRetentionStore::new(temp.path());
        assert_eq!(reopened.pins(id).unwrap(), [pin]);
        assert!(matches!(
            reopened.delete(id, SessionDeletionIntent::Ordinary, || Ok(())),
            Err(StoreError::SessionPinned(_))
        ));
        reopened.release(id, pin).unwrap();
        reopened.release(id, pin).unwrap();
        assert_eq!(
            reopened
                .delete(id, SessionDeletionIntent::Ordinary, || Ok(7))
                .unwrap(),
            7
        );
    }

    #[test]
    fn erase_failure_persists_fence_invalidates_pin_and_rejects_late_publication() {
        let temp = tempfile::tempdir().unwrap();
        let id = crate::new_session_id();
        let pin = SessionRetentionPin(uuid::Uuid::now_v7());
        let store = SessionRetentionStore::new(temp.path());
        store.pin(id, pin, || Ok(true)).unwrap();
        assert!(
            store
                .delete(
                    id,
                    SessionDeletionIntent::ErasePermanently,
                    || Err::<(), _>(std::io::Error::other("injected removal failure").into())
                )
                .is_err()
        );
        let reopened = SessionRetentionStore::new(temp.path());
        assert!(reopened.pins(id).unwrap().is_empty());
        assert!(matches!(
            reopened.pin(id, pin, || Ok(true)),
            Err(StoreError::SessionErased(_))
        ));
        assert!(matches!(
            reopened.lock_live(id),
            Err(StoreError::SessionErased(_))
        ));
        reopened
            .delete(id, SessionDeletionIntent::ErasePermanently, || Ok(()))
            .unwrap();
        reopened.release(id, pin).unwrap();
        assert!(matches!(
            reopened.lock_live(id),
            Err(StoreError::SessionErased(_))
        ));
    }

    #[test]
    fn fs_normal_delete_obeys_pin_and_permanent_erase_fences_all_source_writes() {
        use crate::{FsStore, LogEntry, Store};
        let temp = tempfile::tempdir().unwrap();
        let store = FsStore::new(temp.path()).unwrap();
        let session = crate::new_session_id();
        let segment = crate::new_segment_id();
        let entry = LogEntry::TurnEnd {
            ts: 1,
            turn_count: 1,
        };
        store
            .create_segment(session, segment, &[entry.clone()])
            .unwrap();
        let pin = SessionRetentionPin(uuid::Uuid::now_v7());
        store.pin_session(session, pin).unwrap();
        assert!(matches!(
            store.delete_session(session),
            Err(StoreError::SessionPinned(_))
        ));
        assert!(store.exists(session, segment).unwrap());
        store
            .delete_session_with_intent(session, SessionDeletionIntent::ErasePermanently)
            .unwrap();
        let reopened = FsStore::new(temp.path()).unwrap();
        assert!(!reopened.exists(session, segment).unwrap());
        assert!(matches!(
            reopened.pin_session(session, pin),
            Err(StoreError::SessionErased(_))
        ));
        assert!(matches!(
            reopened.append(session, segment, &entry),
            Err(StoreError::SessionErased(_))
        ));
        assert!(matches!(
            reopened.create_segment(session, segment, &[entry]),
            Err(StoreError::SessionErased(_))
        ));
        assert!(
            !reopened
                .delete_session_with_intent(session, SessionDeletionIntent::ErasePermanently)
                .unwrap()
        );
        assert!(!reopened.exists(session, segment).unwrap());
    }

    #[test]
    fn independent_handles_serialize_delete_with_late_pin() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let control = temp.path().join("control");
        let session = crate::new_session_id();
        let pin = SessionRetentionPin(uuid::Uuid::now_v7());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let control2 = control.clone();
        let source2 = source.clone();
        let delete = std::thread::spawn(move || {
            SessionRetentionStore::new(control2).delete(
                session,
                SessionDeletionIntent::Ordinary,
                || {
                    entered_tx.send(()).unwrap();
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .unwrap();
                    fs::remove_dir(&source2)?;
                    Ok(())
                },
            )
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let late = std::thread::spawn(move || {
            SessionRetentionStore::new(control).pin(session, pin, || Ok(source.is_dir()))
        });
        release_tx.send(()).unwrap();
        delete.join().unwrap().unwrap();
        assert!(matches!(
            late.join().unwrap(),
            Err(StoreError::SessionSourceMissing(_))
        ));
    }

    #[test]
    fn missing_source_cannot_acquire_pin() {
        let temp = tempfile::tempdir().unwrap();
        let id = crate::new_session_id();
        let store = SessionRetentionStore::new(temp.path());
        assert!(matches!(
            store.pin(id, SessionRetentionPin(uuid::Uuid::now_v7()), || Ok(false)),
            Err(StoreError::SessionSourceMissing(_))
        ));
        assert!(store.pins(id).unwrap().is_empty());
    }
}
