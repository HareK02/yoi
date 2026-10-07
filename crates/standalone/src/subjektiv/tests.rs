use super::*;

#[test]
fn catalog_reopen_preserves_random_scope_subject_and_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let catalog = StandaloneSubjects::open(&root).unwrap();
    let subject = catalog
        .create(
            "continuing-owner",
            Some("Keep user behavior separate from Memory."),
        )
        .unwrap();
    let scope = catalog.scope_id().to_owned();
    assert!(uuid::Uuid::parse_str(&scope).is_ok());
    assert!(uuid::Uuid::parse_str(subject.id.strip_prefix("subject-").unwrap()).is_ok());
    assert_ne!(scope, subject.id);
    drop(catalog);
    let reopened = StandaloneSubjects::open_existing(&root).unwrap();
    assert_eq!(reopened.scope_id(), scope);
    assert_eq!(reopened.list().unwrap(), vec![subject]);
}

#[test]
fn implicit_open_does_not_create_subject_storage() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("missing");
    assert!(StandaloneSubjects::open_existing(&root).is_err());
    assert!(!root.exists());
    assert!(StandaloneSubjects::open_existing(temp.path()).is_err());
    assert!(!temp.path().join("subjektiv").exists());
}

#[test]
fn subject_kernel_lease_is_not_worker_or_profile_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = StandaloneSubjects::open(temp.path().join("state")).unwrap();
    let a = catalog.create("a", None).unwrap();
    let b = catalog.create("b", None).unwrap();
    let mut first = catalog
        .connect(&a.id, protocol::WorkerId::now_v7())
        .unwrap();
    assert!(matches!(
        StandaloneSubjects::open_existing(&temp.path().join("state"))
            .unwrap()
            .connect(&a.id, protocol::WorkerId::now_v7()),
        Err(SubjectError::Active(_))
    ));
    let mut second = StandaloneSubjects::open_existing(&temp.path().join("state"))
        .unwrap()
        .connect(&b.id, protocol::WorkerId::now_v7())
        .unwrap();
    first.close().unwrap();
    let mut replacement = StandaloneSubjects::open_existing(&temp.path().join("state"))
        .unwrap()
        .connect(&a.id, protocol::WorkerId::now_v7())
        .unwrap();
    replacement.close().unwrap();
    second.close().unwrap();
}

#[test]
fn closed_capabilities_and_foreign_worker_context_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = StandaloneSubjects::open(temp.path().join("state")).unwrap();
    let subject = catalog.create("owner", None).unwrap();
    let id = protocol::WorkerId::now_v7();
    let mut connection = catalog.connect(&subject.id, id).unwrap();
    let port = connection.host.clone();
    let foreign = SubjektivHostContext {
        worker_id: protocol::WorkerId::now_v7().to_string(),
        session_id: session_store::SessionId::now_v7().to_string(),
    };
    let read = SubjektivMemoryBackendOperation::ResidentContext(
        memory::backend::MemoryResidentSummaryOperation {},
    );
    assert!(port.memory(&foreign, read.clone()).is_err());
    let owner = SubjektivHostContext {
        worker_id: format!("standalone-{id}"),
        session_id: session_store::SessionId::now_v7().to_string(),
    };
    assert!(port.memory(&owner, read.clone()).is_ok());
    assert!(
        port.record_session(&owner, false).is_err(),
        "unattributed Session is not read authority"
    );
    connection.close().unwrap();
    assert!(port.memory(&owner, read).is_err());
}

#[cfg(unix)]
#[test]
fn scope_rejects_symlinks_and_non_private_roots() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let alias = temp.path().join("alias");
    let real = temp.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&real, &alias).unwrap();
    assert!(StandaloneSubjects::open(&alias).is_err());
    let public = temp.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(StandaloneSubjects::open(&public).is_err());
    let catalog = StandaloneSubjects::open(&real).unwrap();
    drop(catalog);
    let scope = real.join("subjektiv/scope.json");
    let original = fs::read(&scope).unwrap();
    fs::remove_file(&scope).unwrap();
    let foreign = temp.path().join("foreign.json");
    fs::write(&foreign, &original).unwrap();
    symlink(&foreign, &scope).unwrap();
    assert!(StandaloneSubjects::open_existing(&real).is_err());
    assert_eq!(fs::read(&foreign).unwrap(), original);
}

#[test]
fn different_local_roots_do_not_authorize_shared_subject_ids() {
    let temp = tempfile::tempdir().unwrap();
    let first = StandaloneSubjects::open(temp.path().join("first")).unwrap();
    let subject = first.create("owner", None).unwrap();
    let other = StandaloneSubjects::open(temp.path().join("other")).unwrap();
    assert_ne!(first.scope_id(), other.scope_id());
    assert!(
        other
            .connect(&subject.id, protocol::WorkerId::now_v7())
            .is_err()
    );
}
