//! Trusted Host placement and lifecycle for Workspace Drive (no HTTP/DTO surface).
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex;
use workspace_drive::Drive;

use crate::{Error, Result, WorkspaceFeatureStorage};

pub(crate) fn drive_blob_root(data_root: &Path, workspace_id: &str) -> Result<PathBuf> {
    if !data_root.is_absolute() {
        return Err(Error::Config(
            "Server data_root must be an absolute path".into(),
        ));
    }
    if workspace_id.is_empty()
        || workspace_id.len() > 128
        || !workspace_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::Config(
            "Drive requires an internal Workspace storage identifier".into(),
        ));
    }
    Ok(data_root.join("drives").join(workspace_id).join("blobs"))
}

// Reject redirected Host-managed ancestors before opening or deleting storage.
// These directories are server-owned, not writable by Drive API clients.
fn reject_symlink_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Error::Config(
                    "Drive storage path contains a symbolic link".into(),
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(Error::Config(
                    "Drive storage path contains a non-directory".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) struct WorkspaceDriveHost {
    registration: crate::RegisteredFeature,
    purged: AtomicBool,
    drive: Mutex<Option<Drive>>,
}

impl WorkspaceDriveHost {
    pub(crate) async fn new(storage: &WorkspaceFeatureStorage) -> Result<Self> {
        let storage = storage.clone();
        let registration = tokio::task::spawn_blocking(move || Drive::register(&storage))
            .await
            .map_err(|error| Error::Store(format!("Drive registration task failed: {error}")))?
            .map_err(|error| Error::Store(format!("failed to register Drive storage: {error}")))?;
        Ok(Self {
            registration,
            purged: AtomicBool::new(false),
            drive: Mutex::new(None),
        })
    }
    pub(crate) async fn open(
        &self,
        storage: &WorkspaceFeatureStorage,
        root: PathBuf,
        authority_database: PathBuf,
    ) -> Result<Drive> {
        let mut cached = self.drive.lock().await;
        if let Some(drive) = cached.as_ref() {
            return Ok(drive.clone());
        }
        let storage = storage.clone();
        let registration = self.registration.clone();
        let drive = tokio::task::spawn_blocking(move || {
            reject_symlink_ancestors(&root)?;
            // Domain IMMEDIATE transactions lock the attached server authority
            // as well as Drive metadata. Deletion reservation and cached-handle
            // operations therefore serialize with other server DB writes.
            Drive::open_with_workspace_authority(
                &storage,
                &registration,
                &root,
                &authority_database,
            )
            .map_err(Error::from)
        })
        .await
        .map_err(|error| Error::Store(format!("Drive open task failed: {error}")))??;
        *cached = Some(drive.clone());
        Ok(drive)
    }

    pub(crate) async fn fence(
        &self,
        storage: &WorkspaceFeatureStorage,
        root: PathBuf,
        authority_database: PathBuf,
    ) -> Result<()> {
        if self.purged.load(Ordering::Acquire) {
            return Ok(());
        }
        let drive = self.open(storage, root, authority_database).await?;
        tokio::task::spawn_blocking(move || drive.fence().map_err(Error::from))
            .await
            .map_err(|error| Error::Store(format!("Drive fence task failed: {error}")))?
    }

    pub(crate) async fn purge(
        &self,
        storage: &WorkspaceFeatureStorage,
        root: PathBuf,
        authority_database: PathBuf,
    ) -> Result<()> {
        if self.purged.load(Ordering::Acquire) {
            return Ok(());
        }
        let drive = self.open(storage, root.clone(), authority_database).await?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            reject_symlink_ancestors(&root)?;
            // Domain purge persists its tombstone before removing blobs/nodes/receipts.
            drive.purge()?;
            // Remove the empty blob root, never the data root or another Workspace.
            // Missing roots are successful retries; other failures retain FeatureStorage.
            match std::fs::remove_dir_all(&root) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        })
        .await
        .map_err(|error| Error::Store(format!("Drive purge task failed: {error}")))??;
        // A later signing-material/finalization failure can retry after the
        // feature DB has been closed and removed by FeatureStorage.delete.
        self.purged.store(true, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workspace_drive::Mutation;

    fn seed_authority(data_root: &Path) -> PathBuf {
        let path = data_root.join("server.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE workspaces (workspace_id TEXT PRIMARY KEY, state TEXT NOT NULL);
            INSERT INTO workspaces VALUES ('workspace-a', 'active');",
        )
        .unwrap();
        path
    }

    #[test]
    fn drive_path_uses_explicit_data_root_and_internal_workspace_id() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        assert_eq!(
            drive_blob_root(&data, "workspace-a").unwrap(),
            data.join("drives/workspace-a/blobs")
        );
        for id in [
            "",
            ".",
            "..",
            "../other",
            "/absolute",
            "a/b",
            "a\\b",
            "display name",
        ] {
            assert!(drive_blob_root(&data, id).is_err(), "accepted {id:?}");
        }
        assert!(drive_blob_root(Path::new("relative"), "workspace-a").is_err());
    }

    #[tokio::test]
    async fn drive_fence_survives_host_reconstruction_before_purge() {
        let temp = tempfile::tempdir().unwrap();
        let feature_root = temp.path().join("feature-storage");
        let root = drive_blob_root(temp.path(), "workspace-a").unwrap();
        let authority = seed_authority(temp.path());
        let storage = crate::FeatureStorage::new(&feature_root)
            .workspace("workspace-a")
            .unwrap();
        let host = WorkspaceDriveHost::new(&storage).await.unwrap();
        let drive = host
            .open(&storage, root.clone(), authority.clone())
            .await
            .unwrap();
        let parent = tokio::task::spawn_blocking(move || drive.root().unwrap().id)
            .await
            .unwrap();
        host.fence(&storage, root.clone(), authority.clone())
            .await
            .unwrap();
        drop(host);
        drop(storage);
        let storage = crate::FeatureStorage::new(&feature_root)
            .workspace("workspace-a")
            .unwrap();
        let host = WorkspaceDriveHost::new(&storage).await.unwrap();
        let reopened = host.open(&storage, root, authority.clone()).await.unwrap();
        let result = tokio::task::spawn_blocking(move || {
            reopened.mutate(
                "after-restart",
                "owner",
                Mutation::CreateFolder {
                    parent,
                    name: "forbidden".into(),
                },
            )
        })
        .await
        .unwrap();
        assert!(
            matches!(result, Err(workspace_drive::Error::Fenced)),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn drive_purge_removes_only_its_blob_root_and_retries_after_feature_delete() {
        let temp = tempfile::tempdir().unwrap();
        let feature_root = temp.path().join("feature-storage");
        let root = drive_blob_root(temp.path(), "workspace-a").unwrap();
        let authority = seed_authority(temp.path());
        let other_root = drive_blob_root(temp.path(), "workspace-b").unwrap();
        std::fs::create_dir_all(&other_root).unwrap();
        std::fs::write(other_root.join("keep"), b"other workspace").unwrap();
        let storage = crate::FeatureStorage::new(&feature_root)
            .workspace("workspace-a")
            .unwrap();
        let host = WorkspaceDriveHost::new(&storage).await.unwrap();
        let drive = host
            .open(&storage, root.clone(), authority.clone())
            .await
            .unwrap();
        tokio::task::spawn_blocking(move || {
            let parent = drive.root().unwrap().id;
            drive
                .mutate(
                    "upload",
                    "owner",
                    Mutation::CreateFile {
                        parent,
                        name: "file".into(),
                        content_type: "text/plain".into(),
                        bytes: b"content".to_vec(),
                    },
                )
                .unwrap();
        })
        .await
        .unwrap();
        host.purge(&storage, root.clone(), authority.clone())
            .await
            .unwrap();
        assert!(!root.exists());
        assert_eq!(
            std::fs::read(other_root.join("keep")).unwrap(),
            b"other workspace"
        );
        let feature_exists = feature_root.join("workspace-a/features/workspace-drive.sqlite");
        assert!(
            feature_exists.exists(),
            "purge must retain the fenced DB: {}",
            feature_exists.display()
        );
        let storage_for_delete = storage.clone();
        tokio::task::spawn_blocking(move || storage_for_delete.delete())
            .await
            .unwrap()
            .unwrap();
        host.fence(&storage, root.clone(), authority.clone())
            .await
            .unwrap();
        host.purge(&storage, root.clone(), authority.clone())
            .await
            .unwrap();
        assert!(!root.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn redirected_drive_root_blocks_cleanup_and_preserves_external_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = drive_blob_root(temp.path(), "workspace-a").unwrap();
        let authority = seed_authority(temp.path());
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep"), b"external").unwrap();
        let storage = crate::FeatureStorage::new(temp.path().join("feature-storage"))
            .workspace("workspace-a")
            .unwrap();
        let host = WorkspaceDriveHost::new(&storage).await.unwrap();
        host.fence(&storage, root.clone(), authority.clone())
            .await
            .unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&outside, &root).unwrap();
        assert!(host.purge(&storage, root, authority).await.is_err());
        assert_eq!(std::fs::read(outside.join("keep")).unwrap(), b"external");
    }
}
