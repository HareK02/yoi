//! Component tests use the production manager, lifecycle locks and filesystem.
//! Only directory sync failure is injected; DB identity checks remain production.
use super::*;
use std::os::unix::fs::symlink;

fn sync_paths(lifecycle: &Lifecycle) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    drop(
        lifecycle
            .lock_with_sync(false, &mut |path| {
                paths.push(path.into());
                sync_directory(path)
            })
            .unwrap(),
    );
    paths
}

#[test]
fn failed_establishment_retries_every_link_even_when_directories_and_lock_exist() {
    let reference = tempfile::tempdir().unwrap();
    let reference_manager = FeatureStorage::new(reference.path().join("nested/storage"));
    let count = sync_paths(&reference_manager.lifecycle("scope")).len();
    assert!(count > 2, "ancestor chain and final lock-directory sync");
    for fail_at in 0..count {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("nested/storage");
        let manager = FeatureStorage::new(&root);
        let lifecycle = manager.lifecycle("scope");
        let mut calls = 0;
        let result = lifecycle.lock_with_sync(false, &mut |path| {
            let index = calls;
            calls += 1;
            if index == fail_at {
                Err(std::io::Error::other("injected directory sync failure"))
            } else {
                sync_directory(path)
            }
        });
        assert!(
            matches!(result, Err(FeatureStorageError::Io(_))),
            "{fail_at}"
        );
        assert_eq!(calls, fail_at + 1);
        assert!(root.join(".lifecycle/scope.lock").is_file());
        assert!(lifecycle.established.scopes.lock().unwrap().is_empty());
        let retried = sync_paths(&lifecycle);
        assert_eq!(
            retried.len(),
            count,
            "all durability steps retry at {fail_at}"
        );
        assert_eq!(
            retried.first(),
            retried.last(),
            "lock directory persisted last"
        );
        drop(
            lifecycle
                .lock_with_sync(false, &mut |_| panic!("already established"))
                .unwrap(),
        );
    }
}

#[test]
fn establishment_is_shared_with_handles_but_not_new_managers_or_scopes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage");
    let manager = FeatureStorage::new(&root);
    let paths = sync_paths(&manager.lifecycle("scope"));
    assert!(!paths.is_empty());
    assert!(sync_paths(&manager.clone().lifecycle("scope")).is_empty());
    assert_eq!(sync_paths(&manager.lifecycle("other")), paths);
    let other = FeatureStorage::new(&root);
    assert_eq!(
        sync_paths(&other.lifecycle("scope")),
        paths,
        "existence is not proof"
    );
    assert_eq!(
        sync_paths(&FeatureStorage::new(&root).lifecycle("scope")),
        paths
    );
}

fn open_database(root: &Path) -> (FeatureStorage, RegisteredFeature, FeatureDatabase) {
    let manager = FeatureStorage::new(root);
    let registration = manager
        .register(FeatureRegistration::new("feature", &[]))
        .unwrap();
    let database = manager.scope("scope").unwrap().open(&registration).unwrap();
    database
        .transaction(|tx| {
            tx.execute_batch(
                "CREATE TABLE marker(value TEXT); INSERT INTO marker VALUES('persisted');",
            )?;
            Ok(())
        })
        .unwrap();
    (manager, registration, database)
}

fn rejected(
    manager: &FeatureStorage,
    registration: &RegisteredFeature,
    database: &FeatureDatabase,
) {
    assert!(
        database
            .with_connection(|_| -> Result<()> { panic!("replaced lock cannot enter DB closure") })
            .is_err()
    );
    assert!(database.guard(database.identity()).is_err());
    assert!(manager.scope("scope").unwrap().open(registration).is_err());
}

#[test]
fn physical_root_directory_lock_and_ancestor_replacement_reject_retained_access() {
    for replacement in ["root", "directory", "lock", "ancestor"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("parent/storage");
        let (manager, registration, database) = open_database(&root);
        let path = match replacement {
            "root" => root.clone(),
            "directory" => root.join(".lifecycle"),
            "lock" => root.join(".lifecycle/scope.lock"),
            "ancestor" => root.parent().unwrap().to_path_buf(),
            _ => unreachable!(),
        };
        let moved = temp.path().join("moved");
        fs::rename(&path, &moved).unwrap();
        if replacement == "lock" {
            fs::write(&path, b"").unwrap();
        } else {
            fs::create_dir_all(&path).unwrap();
        }
        rejected(&manager, &registration, &database);
        // Rejecting must not erase the old proof and accept the replacement on retry.
        rejected(&manager, &registration, &database);
        fs::remove_file(&path)
            .or_else(|_| fs::remove_dir_all(&path))
            .unwrap();
        fs::rename(&moved, &path).unwrap();
        let value: String = database
            .with_connection(|connection| {
                Ok(connection.query_row("SELECT value FROM marker", [], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(value, "persisted");
    }
}

#[test]
fn symlink_replacement_cannot_redirect_or_split_an_established_lock() {
    for replacement in ["root", "directory", "lock"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        let (manager, registration, database) = open_database(&root);
        let path = match replacement {
            "root" => root.clone(),
            "directory" => root.join(".lifecycle"),
            "lock" => root.join(".lifecycle/scope.lock"),
            _ => unreachable!(),
        };
        let moved = temp.path().join("moved");
        fs::rename(&path, &moved).unwrap();
        symlink(&moved, &path).unwrap();
        // The target inode is still the original; following metadata alone is insufficient.
        rejected(&manager, &registration, &database);
        fs::remove_file(&path).unwrap();
        fs::rename(&moved, &path).unwrap();
        drop(database.guard(database.identity()).unwrap());
    }
}

#[test]
fn initial_lock_namespace_symlinks_are_rejected_without_establishment_proof() {
    for directory_link in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        fs::create_dir(&root).unwrap();
        let target = temp.path().join("target");
        if directory_link {
            fs::create_dir(&target).unwrap();
            symlink(&target, root.join(".lifecycle")).unwrap();
        } else {
            fs::create_dir(root.join(".lifecycle")).unwrap();
            fs::write(&target, b"").unwrap();
            symlink(&target, root.join(".lifecycle/scope.lock")).unwrap();
        }
        let manager = FeatureStorage::new(&root);
        let lifecycle = manager.lifecycle("scope");
        assert!(matches!(
            lifecycle.lock(false),
            Err(FeatureStorageError::Identity(_))
        ));
        assert!(lifecycle.established.scopes.lock().unwrap().is_empty());
    }
}

#[test]
fn configured_host_symlinks_reestablish_and_reject_changed_canonical_root() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    fs::create_dir(&target).unwrap();
    let root = temp.path().join("alias");
    symlink(&target, &root).unwrap();
    let manager = FeatureStorage::new(&root);
    let lifecycle = manager.lifecycle("scope");
    let paths = sync_paths(&lifecycle);
    assert!(paths.contains(&target));
    // Mutable symlink links are not pinned by ordinary directory descriptors;
    // therefore no successful proof can suppress their durability steps.
    assert_eq!(sync_paths(&lifecycle), paths);
    let next = temp.path().join("next");
    fs::create_dir(&next).unwrap();
    fs::remove_file(&root).unwrap();
    symlink(&next, &root).unwrap();
    assert!(matches!(
        lifecycle.lock(false),
        Err(FeatureStorageError::Identity(_))
    ));
}

#[cfg(feature = "test-support")]
#[test]
fn replacement_during_os_lock_wait_is_rejected_before_database_access() {
    use std::time::{Duration, Instant};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage");
    let (manager, registration, database) = open_database(&root);
    let guard = database.guard(database.identity()).unwrap();
    let attempts = manager.lifecycle_observation().lock_attempts;
    let lifecycle = manager.lifecycle("scope");
    let waiter = std::thread::spawn(move || lifecycle.lock(true));
    let deadline = Instant::now() + Duration::from_secs(5);
    while manager.lifecycle_observation().lock_attempts == attempts {
        assert!(
            Instant::now() < deadline,
            "waiter did not reach OS lock call"
        );
        std::thread::yield_now();
    }
    let lock_path = root.join(".lifecycle/scope.lock");
    fs::rename(&lock_path, root.join(".lifecycle/held.lock")).unwrap();
    fs::write(&lock_path, b"").unwrap();
    drop(guard);
    assert!(matches!(
        waiter.join().unwrap(),
        Err(FeatureStorageError::Identity(_))
    ));
    rejected(&manager, &registration, &database);
}
