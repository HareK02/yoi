//! Request-time checkout contracts. Validators are opaque provider observations,
//! not content hashes. Callers must separately retain Read's content hash for
//! read-before-mutation. Create destinations are checkout-root relative.
use crate::{
    ContentHash, EditResult, EntryKind, ReadResult, WorkdirError, WorkdirPath,
    WorkdirSessionCapabilities, WorkdirSessionCapability, WriteResult,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckoutObservation {
    pub path: WorkdirPath,
    pub kind: EntryKind,
    pub size: u64,
    pub validator: Vec<u8>,
    pub capabilities: WorkdirSessionCapabilities,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckoutRequest {
    pub target: WorkdirPath,
    pub validator: Vec<u8>,
    pub operation: CheckoutOperation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckoutOperation {
    Read {
        offset: usize,
        limit: usize,
        max_bytes: usize,
    },
    Write {
        content: Vec<u8>,
        expected_hash: ContentHash,
    },
    Edit {
        old_string: String,
        new_string: String,
        replace_all: bool,
        expected_hash: ContentHash,
    },
    Create {
        path: WorkdirPath,
        content: Vec<u8>,
    },
}
impl CheckoutOperation {
    pub fn capability(&self) -> WorkdirSessionCapability {
        match self {
            Self::Read { .. } => WorkdirSessionCapability::Read,
            Self::Edit { .. } => WorkdirSessionCapability::Edit,
            Self::Write { .. } | Self::Create { .. } => WorkdirSessionCapability::Write,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "operation",
    content = "result",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CheckoutOutput {
    Read(ReadResult),
    Write(WriteResult),
    Edit(EditResult),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckoutResult {
    /// Post-operation state of the bound request target. For Create this is the
    /// parent directory, not the destination file (whose path the caller owns).
    pub observation: CheckoutObservation,
    pub output: CheckoutOutput,
}

/// One provider-side traversal. Rules union within a layer and intersect across
/// layers, current provider authority and output_root. Wrappers add layers;
/// callers normally use new() and never supply provider coordinates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckoutSearchRequest {
    pub operation: CheckoutSearchOperation,
    pub scope_layers: Vec<Vec<crate::WorkdirToolScopeRule>>,
    pub output_root: WorkdirPath,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "operation",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CheckoutSearchOperation {
    List(crate::ListRequest),
    Glob(crate::GlobRequest),
    Grep(crate::GrepRequest),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "operation",
    content = "result",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CheckoutSearchResult {
    List(crate::ListResult),
    Glob(crate::GlobResult),
    Grep(crate::GrepResult),
}
impl CheckoutSearchOperation {
    pub fn path(&self) -> &WorkdirPath {
        match self {
            Self::List(r) => &r.path,
            Self::Glob(r) => &r.path,
            Self::Grep(r) => &r.path,
        }
    }
    pub fn path_mut(&mut self) -> &mut WorkdirPath {
        match self {
            Self::List(r) => &mut r.path,
            Self::Glob(r) => &mut r.path,
            Self::Grep(r) => &mut r.path,
        }
    }
    pub fn capability(&self) -> WorkdirSessionCapability {
        match self {
            Self::List(_) => WorkdirSessionCapability::Read,
            Self::Glob(_) => WorkdirSessionCapability::Glob,
            Self::Grep(_) => WorkdirSessionCapability::Grep,
        }
    }
}
impl CheckoutSearchRequest {
    pub fn new(operation: CheckoutSearchOperation) -> Self {
        Self {
            operation,
            scope_layers: Vec::new(),
            output_root: WorkdirPath::root(),
        }
    }
    pub(crate) fn validate(&self) -> Result<(), WorkdirError> {
        validate_path(&self.output_root)?;
        validate_path(self.operation.path())?;
        if !std::path::Path::new(self.operation.path().as_str())
            .starts_with(self.output_root.as_str())
        {
            return Err(WorkdirError::Denied(
                "checkout search must stay beneath its output root".into(),
            ));
        }
        if self.scope_layers.len() > 16
            || self.scope_layers.iter().map(Vec::len).sum::<usize>() > 256
            || self.scope_layers.iter().any(Vec::is_empty)
        {
            return Err(WorkdirError::InvalidArgument(
                "checkout search scope exceeds provider bounds".into(),
            ));
        }
        for layer in &self.scope_layers {
            for rule in layer {
                validate_path(&rule.target)?;
            }
        }
        let valid = match &self.operation {
            CheckoutSearchOperation::List(r) => {
                r.limit <= crate::external::MAX_EXTERNAL_RESULT_ITEMS
            }
            CheckoutSearchOperation::Glob(r) => {
                r.limit <= crate::external::MAX_EXTERNAL_RESULT_ITEMS && r.pattern.len() <= 4096
            }
            CheckoutSearchOperation::Grep(r) => {
                r.limit <= crate::external::MAX_EXTERNAL_RESULT_ITEMS
                    && r.offset <= 1_000_000
                    && r.pattern.len() <= 4096
                    && r.glob.as_ref().is_none_or(|s| s.len() <= 4096)
                    && r.file_type.as_ref().is_none_or(|s| s.len() <= 128)
                    && r.before_context <= 100
                    && r.after_context <= 100
            }
        };
        if valid {
            Ok(())
        } else {
            Err(WorkdirError::InvalidArgument(
                "checkout search exceeds provider bounds".into(),
            ))
        }
    }
}

pub(crate) fn validate_path(path: &WorkdirPath) -> Result<(), WorkdirError> {
    WorkdirPath::new(path.as_str())?;
    if path.as_str().len() > 4096 || std::path::Path::new(path.as_str()).components().count() > 128
    {
        return Err(WorkdirError::InvalidArgument(
            "checkout path exceeds provider bounds".into(),
        ));
    }
    Ok(())
}

/// Check strict containment without converting a directory-relative path into
/// ambient authority. Absolute paths are forbidden even if otherwise scoped.
pub fn create_relative_path(
    target: &WorkdirPath,
    path: &WorkdirPath,
) -> Result<std::path::PathBuf, WorkdirError> {
    validate_path(target)?;
    validate_path(path)?;
    let target = std::path::Path::new(target.as_str());
    let path = std::path::Path::new(path.as_str());
    if target.is_absolute() || path.is_absolute() {
        return Err(WorkdirError::InvalidArgument(
            "checkout paths must be root relative".into(),
        ));
    }
    let relative = path.strip_prefix(target).map_err(|_| {
        WorkdirError::InvalidArgument(
            "Create destination must be strictly beneath its target".into(),
        )
    })?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(WorkdirError::InvalidArgument(
            "Create destination must be strictly beneath its target".into(),
        ));
    }
    Ok(relative.to_path_buf())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{LocalWorkdirSession, WorkdirSession};
    use std::sync::Arc;
    fn path(p: &str) -> WorkdirPath {
        WorkdirPath::new(p).unwrap()
    }
    fn session(root: &std::path::Path) -> LocalWorkdirSession {
        LocalWorkdirSession::new(manifest::Scope::writable(root).unwrap(), root.to_path_buf())
    }
    async fn checked_read(
        session: &dyn WorkdirSession,
        observation: CheckoutObservation,
    ) -> CheckoutResult {
        session
            .checkout_execute(CheckoutRequest {
                target: observation.path,
                validator: observation.validator,
                operation: CheckoutOperation::Read {
                    offset: 0,
                    limit: 20,
                    max_bytes: 1024,
                },
            })
            .await
            .unwrap()
    }
    fn hash(result: &CheckoutResult) -> ContentHash {
        match &result.output {
            CheckoutOutput::Read(r) => r.content_hash,
            _ => panic!("not Read"),
        }
    }
    #[tokio::test]
    async fn checkout_read_edit_write_create_share_hash_and_string_contracts() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), "alpha\nbeta\n").unwrap();
        let session = session(root.path());
        let read = checked_read(&session, session.checkout_observe(path("a")).await.unwrap()).await;
        let edited = session
            .checkout_execute(CheckoutRequest {
                target: path("a"),
                validator: read.observation.validator.clone(),
                operation: CheckoutOperation::Edit {
                    old_string: "beta".into(),
                    new_string: "gamma".into(),
                    replace_all: false,
                    expected_hash: hash(&read),
                },
            })
            .await
            .unwrap();
        assert!(matches!(
            edited.output,
            CheckoutOutput::Edit(EditResult {
                replacements: 1,
                ..
            })
        ));
        assert_eq!(
            std::fs::read_to_string(root.path().join("a")).unwrap(),
            "alpha\ngamma\n"
        );
        let read = checked_read(&session, edited.observation).await;
        // A validator alone does not bypass read-history's expected content hash.
        assert!(matches!(
            session
                .checkout_execute(CheckoutRequest {
                    target: path("a"),
                    validator: read.observation.validator.clone(),
                    operation: CheckoutOperation::Write {
                        content: b"bad".to_vec(),
                        expected_hash: [0; 32]
                    }
                })
                .await,
            Err(WorkdirError::Conflict(_))
        ));
        let written = session
            .checkout_execute(CheckoutRequest {
                target: path("a"),
                validator: read.observation.validator.clone(),
                operation: CheckoutOperation::Write {
                    content: b"saved".to_vec(),
                    expected_hash: hash(&read),
                },
            })
            .await
            .unwrap();
        assert_ne!(written.observation.validator, read.observation.validator);
        let directory = session.checkout_observe(WorkdirPath::root()).await.unwrap();
        let created = session
            .checkout_execute(CheckoutRequest {
                target: directory.path,
                validator: directory.validator,
                operation: CheckoutOperation::Create {
                    path: path("nested/new"),
                    content: b"new".to_vec(),
                },
            })
            .await
            .unwrap();
        assert_eq!(created.observation.path, WorkdirPath::root());
        assert_eq!(created.observation.kind, EntryKind::Directory);
        assert_eq!(
            created.observation,
            session.checkout_observe(WorkdirPath::root()).await.unwrap()
        );
        assert_eq!(
            std::fs::read(root.path().join("nested/new")).unwrap(),
            b"new"
        );
        assert!(matches!(
            created.output,
            CheckoutOutput::Write(WriteResult { created: true, .. })
        ));
    }
    #[tokio::test]
    async fn checkout_observe_missing_routes_are_not_found_but_deleted_execution_is_conflict() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), "original").unwrap();
        let local = session(root.path());
        let external = LocalWorkdirSession::external_read_write(
            crate::Workdir::new("external-missing"),
            root.path(),
            crate::BoundedReadLimits::EXTERNAL_DEFAULT,
        )
        .unwrap();
        let scoped = crate::WorkdirToolBroker::new(Arc::new(local.clone()));
        for provider in [
            &local as &dyn WorkdirSession,
            &external,
            &*scoped.tool_session(),
        ] {
            for missing in ["missing", "missing-parent/child", "file/child"] {
                let error = provider.checkout_observe(path(missing)).await.unwrap_err();
                assert!(
                    matches!(&error, WorkdirError::NotFound(logical) if logical == std::path::Path::new(missing)),
                    "{missing}: {error:?}"
                );
                assert_eq!(
                    crate::http::WorkdirTransportError::from_workdir_error(&error).code,
                    crate::http::WorkdirTransportErrorCode::NotFound
                );
            }
            std::fs::write(root.path().join("observed"), "original").unwrap();
            let read = checked_read(
                provider,
                provider.checkout_observe(path("observed")).await.unwrap(),
            )
            .await;
            std::fs::remove_file(root.path().join("observed")).unwrap();
            for operation in [
                CheckoutOperation::Read {
                    offset: 0,
                    limit: 10,
                    max_bytes: 100,
                },
                CheckoutOperation::Write {
                    content: b"new".to_vec(),
                    expected_hash: hash(&read),
                },
                CheckoutOperation::Edit {
                    old_string: "original".into(),
                    new_string: "new".into(),
                    replace_all: false,
                    expected_hash: hash(&read),
                },
            ] {
                let error = provider
                    .checkout_execute(CheckoutRequest {
                        target: path("observed"),
                        validator: read.observation.validator.clone(),
                        operation,
                    })
                    .await
                    .unwrap_err();
                assert!(matches!(error, WorkdirError::Conflict(_)), "{error:?}");
                assert_eq!(
                    crate::http::WorkdirTransportError::from_workdir_error(&error).code,
                    crate::http::WorkdirTransportErrorCode::Conflict
                );
            }
            assert!(matches!(
                provider.checkout_observe(path("observed")).await,
                Err(WorkdirError::NotFound(_))
            ));
            std::fs::create_dir(root.path().join("directory")).unwrap();
            let directory = provider.checkout_observe(path("directory")).await.unwrap();
            std::fs::remove_dir(root.path().join("directory")).unwrap();
            let error = provider
                .checkout_execute(CheckoutRequest {
                    target: directory.path,
                    validator: directory.validator,
                    operation: CheckoutOperation::Create {
                        path: path("directory/new"),
                        content: b"new".to_vec(),
                    },
                })
                .await
                .unwrap_err();
            assert!(matches!(error, WorkdirError::Conflict(_)));
            assert!(!root.path().join("directory").exists());
            // An observed parent replaced with a regular file makes the old
            // descendant execution stale, while discovery reports no route.
            std::fs::create_dir(root.path().join("parent")).unwrap();
            std::fs::write(root.path().join("parent/child"), "original").unwrap();
            let child = provider
                .checkout_observe(path("parent/child"))
                .await
                .unwrap();
            std::fs::remove_dir_all(root.path().join("parent")).unwrap();
            std::fs::write(root.path().join("parent"), "replacement").unwrap();
            assert!(matches!(
                provider.checkout_observe(path("parent/child")).await,
                Err(WorkdirError::NotFound(_))
            ));
            assert!(matches!(
                provider
                    .checkout_execute(CheckoutRequest {
                        target: child.path,
                        validator: child.validator,
                        operation: CheckoutOperation::Read {
                            offset: 0,
                            limit: 10,
                            max_bytes: 100
                        }
                    })
                    .await,
                Err(WorkdirError::Conflict(_))
            ));
            std::fs::remove_file(root.path().join("parent")).unwrap();
        }
    }

    #[tokio::test]
    async fn checkout_inode_substitution_and_stale_content_are_conflicts() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), "same").unwrap();
        let session = session(root.path());
        let original =
            checked_read(&session, session.checkout_observe(path("a")).await.unwrap()).await;
        std::fs::rename(root.path().join("a"), root.path().join("old")).unwrap();
        std::fs::write(root.path().join("a"), "same").unwrap();
        let new = session.checkout_observe(path("a")).await.unwrap();
        assert_ne!(original.observation.validator, new.validator);
        assert!(matches!(
            session
                .checkout_execute(CheckoutRequest {
                    target: path("a"),
                    validator: original.observation.validator.clone(),
                    operation: CheckoutOperation::Write {
                        content: b"wrong".to_vec(),
                        expected_hash: hash(&original)
                    }
                })
                .await,
            Err(WorkdirError::Conflict(_))
        ));
        assert_eq!(
            std::fs::read_to_string(root.path().join("a")).unwrap(),
            "same"
        );
        std::fs::write(root.path().join("a"), "changed").unwrap();
        assert!(matches!(
            session
                .checkout_execute(CheckoutRequest {
                    target: path("a"),
                    validator: new.validator,
                    operation: CheckoutOperation::Read {
                        offset: 0,
                        limit: 20,
                        max_bytes: 100
                    }
                })
                .await,
            Err(WorkdirError::Conflict(_))
        ));
    }
    #[tokio::test]
    async fn checkout_hardlink_save_isolates_alias_and_rejects_symlinks_special_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), "original").unwrap();
        std::fs::hard_link(root.path().join("a"), root.path().join("alias")).unwrap();
        std::os::unix::fs::symlink("a", root.path().join("link")).unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(root.path().join("socket")).unwrap();
        let session = session(root.path());
        assert!(session.checkout_observe(path("link")).await.is_err());
        assert!(session.checkout_observe(path("socket")).await.is_err());
        let read = checked_read(&session, session.checkout_observe(path("a")).await.unwrap()).await;
        session
            .checkout_execute(CheckoutRequest {
                target: path("a"),
                validator: read.observation.validator.clone(),
                operation: CheckoutOperation::Write {
                    content: b"saved".to_vec(),
                    expected_hash: hash(&read),
                },
            })
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("alias")).unwrap(),
            "original"
        );
    }
    #[tokio::test]
    async fn checkout_create_is_strictly_beneath_directory_and_does_not_clobber() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("d")).unwrap();
        std::fs::write(root.path().join("d/existing"), "keep").unwrap();
        let session = session(root.path());
        for destination in ["sibling", "d", "directory/file"] {
            let observation = session.checkout_observe(path("d")).await.unwrap();
            assert!(
                session
                    .checkout_execute(CheckoutRequest {
                        target: path("d"),
                        validator: observation.validator,
                        operation: CheckoutOperation::Create {
                            path: path(destination),
                            content: vec![1]
                        }
                    })
                    .await
                    .is_err()
            );
        }
        assert!(WorkdirPath::new("d/../sibling").is_err());
        assert!(
            create_relative_path(&path("d"), &WorkdirPath::new_scoped("/d/new").unwrap()).is_err()
        );
        let observation = session.checkout_observe(path("d")).await.unwrap();
        assert!(
            session
                .checkout_execute(CheckoutRequest {
                    target: path("d"),
                    validator: observation.validator,
                    operation: CheckoutOperation::Create {
                        path: path("d/existing"),
                        content: vec![1]
                    }
                })
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("d/existing")).unwrap(),
            "keep"
        );
        let observation = session.checkout_observe(path("d")).await.unwrap();
        std::fs::write(root.path().join("d/concurrent"), "created externally").unwrap();
        assert!(matches!(
            session
                .checkout_execute(CheckoutRequest {
                    target: path("d"),
                    validator: observation.validator,
                    operation: CheckoutOperation::Create {
                        path: path("d/new"),
                        content: vec![1]
                    }
                })
                .await,
            Err(WorkdirError::Conflict(_))
        ));
        assert!(!root.path().join("d/new").exists());
    }
    #[tokio::test]
    async fn checkout_external_approved_descriptor_survives_root_path_swap() {
        let parent = tempfile::tempdir().unwrap();
        let selected = parent.path().join("selected");
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(selected.join("a"), "approved").unwrap();
        let session = LocalWorkdirSession::external_read_write(
            crate::Workdir::new("external"),
            &selected,
            crate::BoundedReadLimits::EXTERNAL_DEFAULT,
        )
        .unwrap();
        let original = session.checkout_observe(path("a")).await.unwrap();
        let approved = parent.path().join("approved");
        std::fs::rename(&selected, &approved).unwrap();
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(selected.join("a"), "replacement").unwrap();
        let read = checked_read(&session, original).await;
        assert!(matches!(&read.output, CheckoutOutput::Read(r) if r.bytes == b"approved"));
        session
            .checkout_execute(CheckoutRequest {
                target: path("a"),
                validator: read.observation.validator.clone(),
                operation: CheckoutOperation::Write {
                    content: b"saved".to_vec(),
                    expected_hash: hash(&read),
                },
            })
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(approved.join("a")).unwrap(),
            "saved"
        );
        assert_eq!(
            std::fs::read_to_string(selected.join("a")).unwrap(),
            "replacement"
        );
    }
    #[tokio::test]
    async fn checkout_scope_readonly_write_leases_and_revocation_attenuate_authority() {
        use crate::{
            WorkdirToolBroker, WorkdirToolScope, WorkdirToolScopePermission as Permission,
            WorkdirToolScopeRule,
        };
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("child")).unwrap();
        std::fs::write(root.path().join("child/a"), "child").unwrap();
        std::fs::write(root.path().join("outside"), "outside").unwrap();
        let source = Arc::new(session(root.path()));
        let broker = WorkdirToolBroker::new(source.clone());
        let lease = broker
            .scope(WorkdirToolScope {
                rules: vec![WorkdirToolScopeRule {
                    target: path("child"),
                    permission: Permission::Write,
                    recursive: true,
                    symlink_policy: Default::default(),
                }],
                cwd: path("child"),
                command: false,
            })
            .await
            .unwrap();
        let child = lease.tool_session();
        assert!(child.checkout_observe(path("outside")).await.is_err());
        let observation = child.checkout_observe(path("a")).await.unwrap();
        assert!(
            observation
                .capabilities
                .supports(WorkdirSessionCapability::Write)
        );
        let parent_observation = broker.checkout_observe(path("child/a")).await.unwrap();
        assert!(
            !parent_observation
                .capabilities
                .supports(WorkdirSessionCapability::Write)
        );
        let read = checked_read(&*child, observation).await;
        assert!(
            broker
                .checkout_execute(CheckoutRequest {
                    target: path("child/a"),
                    validator: parent_observation.validator,
                    operation: CheckoutOperation::Write {
                        content: b"blocked".to_vec(),
                        expected_hash: hash(&read)
                    }
                })
                .await
                .is_err()
        );
        let readonly = crate::ReadOnlyWorkdirSession::new(child.clone());
        assert!(
            !readonly
                .checkout_observe(path("a"))
                .await
                .unwrap()
                .capabilities
                .supports(WorkdirSessionCapability::Write)
        );
        assert!(
            readonly
                .checkout_execute(CheckoutRequest {
                    target: path("a"),
                    validator: read.observation.validator.clone(),
                    operation: CheckoutOperation::Edit {
                        old_string: "child".into(),
                        new_string: "bad".into(),
                        replace_all: false,
                        expected_hash: hash(&read)
                    }
                })
                .await
                .is_err()
        );
        let directory = child.checkout_observe(WorkdirPath::root()).await.unwrap();
        let created = child
            .checkout_execute(CheckoutRequest {
                target: WorkdirPath::root(),
                validator: directory.validator,
                operation: CheckoutOperation::Create {
                    path: path("fresh"),
                    content: b"fresh".to_vec(),
                },
            })
            .await
            .unwrap();
        assert_eq!(created.observation.path, WorkdirPath::root());
        assert_eq!(created.observation.kind, EntryKind::Directory);
        assert_eq!(
            created.observation,
            child.checkout_observe(WorkdirPath::root()).await.unwrap()
        );
        assert_eq!(
            std::fs::read(root.path().join("child/fresh")).unwrap(),
            b"fresh"
        );
        lease.close().await.unwrap();
        assert!(matches!(
            child
                .checkout_execute(CheckoutRequest {
                    target: path("a"),
                    validator: read.observation.validator.clone(),
                    operation: CheckoutOperation::Read {
                        offset: 0,
                        limit: 10,
                        max_bytes: 100
                    }
                })
                .await,
            Err(WorkdirError::SessionClosed)
        ));
    }
    #[tokio::test]
    async fn shared_search_skips_non_utf8_names_without_colliding_with_unicode_paths() {
        use crate::{GlobRequest, GrepOutputMode, GrepRequest, ListRequest};
        use std::os::unix::ffi::OsStringExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(std::ffi::OsString::from_vec(
                b"replacement\xff.txt".to_vec(),
            )),
            "needle invalid",
        )
        .unwrap();
        std::fs::write(root.path().join("replacement\u{fffd}.txt"), "needle valid").unwrap();
        std::fs::write(root.path().join("é.txt"), "needle composed").unwrap();
        std::fs::write(root.path().join("e\u{301}.txt"), "needle decomposed").unwrap();
        let invalid_dir = root
            .path()
            .join(std::ffi::OsString::from_vec(b"directory\xff".to_vec()));
        std::fs::create_dir(&invalid_dir).unwrap();
        std::fs::write(invalid_dir.join("child.txt"), "needle hidden invalid child").unwrap();
        std::fs::create_dir(root.path().join("directory\u{fffd}")).unwrap();
        std::fs::write(
            root.path().join("directory\u{fffd}/child.txt"),
            "needle visible valid child",
        )
        .unwrap();
        let local = session(root.path());
        let external = LocalWorkdirSession::external_read_only(
            crate::Workdir::new("external-search"),
            root.path(),
            crate::BoundedReadLimits::EXTERNAL_DEFAULT,
        )
        .unwrap();
        for provider in [&local as &dyn WorkdirSession, &external] {
            let listed = provider
                .list(ListRequest {
                    path: WorkdirPath::root(),
                    limit: 100,
                })
                .await
                .unwrap();
            assert_eq!(listed.entries.len(), 4);
            assert_eq!(
                listed
                    .entries
                    .iter()
                    .filter(|e| e.path == path("replacement\u{fffd}.txt"))
                    .count(),
                1
            );
            let globbed = provider
                .glob(GlobRequest {
                    path: WorkdirPath::root(),
                    pattern: "**/*.txt".into(),
                    limit: 100,
                })
                .await
                .unwrap();
            let mut expected = vec![
                path("directory\u{fffd}/child.txt"),
                path("replacement\u{fffd}.txt"),
                path("é.txt"),
                path("e\u{301}.txt"),
            ];
            expected.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            assert_eq!(globbed.paths, expected);
            for mode in [
                GrepOutputMode::Content,
                GrepOutputMode::FilesWithMatches,
                GrepOutputMode::Count,
            ] {
                let grep = provider
                    .grep(GrepRequest {
                        path: WorkdirPath::root(),
                        pattern: "needle".into(),
                        glob: None,
                        file_type: None,
                        case_insensitive: false,
                        before_context: 0,
                        after_context: 0,
                        multiline: false,
                        output_mode: mode,
                        offset: 0,
                        limit: 100,
                    })
                    .await
                    .unwrap();
                let mut paths = grep.paths;
                paths.sort_by(|a, b| a.as_str().cmp(b.as_str()));
                assert_eq!(paths, expected);
                assert_eq!(grep.matched_files, 4);
                assert_eq!(grep.match_count, 4);
                assert!(!grep.output.contains("needle invalid"));
                assert!(!grep.output.contains("hidden invalid child"));
            }
        }
    }

    #[tokio::test]
    async fn shared_grep_typed_paths_follow_returned_modes_offset_limit_and_odd_names() {
        use crate::{GrepOutputMode, GrepRequest};
        let root = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt", "c.txt", "line\nfile:1.txt"] {
            std::fs::write(root.path().join(name), "needle").unwrap();
        }
        let session = session(root.path());
        for mode in [
            GrepOutputMode::Content,
            GrepOutputMode::FilesWithMatches,
            GrepOutputMode::Count,
        ] {
            let request = GrepRequest {
                path: WorkdirPath::root(),
                pattern: "needle".into(),
                glob: None,
                file_type: None,
                case_insensitive: false,
                before_context: 0,
                after_context: 0,
                multiline: false,
                output_mode: mode,
                offset: 1,
                limit: 1,
            };
            let result = session.grep(request.clone()).await.unwrap();
            assert_eq!(result.paths.len(), 1);
            assert_eq!(result.matched_files, 1);
            assert!(result.output.contains(result.paths[0].as_str()));
            let empty = session
                .grep(GrepRequest {
                    limit: 0,
                    ..request.clone()
                })
                .await
                .unwrap();
            assert!(empty.output.is_empty());
            assert!(empty.paths.is_empty());
            let direct = session
                .grep(GrepRequest {
                    path: path("line\nfile:1.txt"),
                    offset: 0,
                    ..request
                })
                .await
                .unwrap();
            assert_eq!(direct.paths, vec![path("line\nfile:1.txt")]);
        }
        let legacy: crate::GrepResult = serde_json::from_value(
            serde_json::json!({"output":"", "match_count":0,"matched_files":0,"truncated":false}),
        )
        .unwrap();
        assert!(legacy.paths.is_empty());
    }

    #[tokio::test]
    async fn checkout_partial_parent_creation_is_outcome_unknown() {
        let root = tempfile::tempdir().unwrap();
        let session = session(root.path());
        let observation = session.checkout_observe(WorkdirPath::root()).await.unwrap();
        // The final name cannot be saved, but an earlier mkdir has effects.
        let destination = path(&format!("made/{}", "x".repeat(300)));
        let error = session
            .checkout_execute(CheckoutRequest {
                target: observation.path,
                validator: observation.validator,
                operation: CheckoutOperation::Create {
                    path: destination,
                    content: b"new".to_vec(),
                },
            })
            .await
            .unwrap_err();
        assert!(matches!(error, WorkdirError::OutcomeUnknown(_)));
        assert!(root.path().join("made").is_dir());
    }

    #[tokio::test]
    async fn checkout_http_dispatch_and_external_dto_bounds() {
        use crate::external::{ExternalWorkdirOperation, ExternalWorkdirOperationResult};
        use crate::http::{
            WorkdirSessionOperation as Op, WorkdirSessionOperationResult as Result,
            WorkdirTransportError, WorkdirTransportErrorCode, dispatch_workdir_session_operation,
        };
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), "contents").unwrap();
        let session = session(root.path());
        let Result::CheckoutObserve(observation) =
            dispatch_workdir_session_operation(&session, Op::CheckoutObserve(path("a")))
                .await
                .unwrap()
        else {
            panic!()
        };
        let request = CheckoutRequest {
            target: path("a"),
            validator: observation.validator,
            operation: CheckoutOperation::Read {
                offset: 0,
                limit: 10,
                max_bytes: 100,
            },
        };
        assert!(ExternalWorkdirOperation::try_from(Op::CheckoutExecute(request.clone())).is_ok());
        let response =
            dispatch_workdir_session_operation(&session, Op::CheckoutExecute(request.clone()))
                .await
                .unwrap();
        assert!(matches!(response, Result::CheckoutExecute(_)));
        assert!(ExternalWorkdirOperationResult::try_from(response.clone()).is_ok());
        assert_eq!(
            serde_json::from_value::<Result>(serde_json::to_value(response).unwrap())
                .unwrap()
                .clone(),
            dispatch_workdir_session_operation(&session, Op::CheckoutExecute(request.clone()))
                .await
                .unwrap()
        );
        let mut oversized = request.clone();
        oversized.validator = vec![0; 257];
        assert!(ExternalWorkdirOperation::try_from(Op::CheckoutExecute(oversized)).is_err());
        let mut oversized = request;
        oversized.operation = CheckoutOperation::Read {
            offset: 0,
            limit: 1,
            max_bytes: 1024 * 1024 + 1,
        };
        assert!(ExternalWorkdirOperation::try_from(Op::CheckoutExecute(oversized)).is_err());
        let grep = crate::GrepResult {
            paths: vec![path("a")],
            output: "a\n".into(),
            match_count: 1,
            matched_files: 1,
            truncated: false,
        };
        assert!(ExternalWorkdirOperationResult::try_from(Result::Grep(grep.clone())).is_ok());
        let invalid = crate::GrepResult {
            paths: vec![WorkdirPath::new_scoped("/outside").unwrap()],
            ..grep.clone()
        };
        assert!(ExternalWorkdirOperationResult::try_from(Result::Grep(invalid)).is_err());
        let oversized = crate::GrepResult {
            paths: vec![path("a"); crate::external::MAX_EXTERNAL_RESULT_ITEMS + 1],
            ..grep
        };
        assert!(ExternalWorkdirOperationResult::try_from(Result::Grep(oversized)).is_err());
        let unknown = WorkdirTransportError::from_workdir_error(&WorkdirError::OutcomeUnknown(
            "/secret/host/path".into(),
        ));
        assert_eq!(unknown.code, WorkdirTransportErrorCode::OutcomeUnknown);
        assert!(!unknown.message.contains("secret"));
        assert!(matches!(
            unknown.into_workdir_error(),
            WorkdirError::OutcomeUnknown(_)
        ));
    }
}
