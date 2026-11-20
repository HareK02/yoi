//! Publication has a durable no-retry boundary before dispatch. A provider
//! acknowledgement is retained independently of subsequent ref positions. Ref
//! equality alone cannot prove who published: recovery never manufactures a
//! receipt from a moved branch and never sends a second push.
use super::*;
use crate::catalog::{
    RepositoryPublicationBinding, RepositoryPublicationOutcome as Outcome,
    RepositoryPublicationReceipt as Receipt, RepositoryPublicationRequest as Request,
};

fn denied(message: impl Into<String>) -> WorkingDirectoryDiagnostic {
    WorkingDirectoryDiagnostic::rejected("repository_publication_rejected", message)
}

// Do not inherit a caller Git directory, replacement graph, config injection,
// or ambient SSH identity into a fixed Repository operation.
fn publication_git_command(
    request: &WorkingDirectoryRequest,
    access: Option<&RepositoryCommandAccess>,
) -> Command {
    let mut command = repository_git_command(request, access);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_SHALLOW_FILE",
        "GIT_REPLACE_REF_BASE",
        "GIT_EXEC_PATH",
        "GIT_SSH",
    ] {
        command.env_remove(key);
    }
    if access.is_none() {
        command
            .env_remove("GIT_SSH_COMMAND")
            .env_remove("SSH_AUTH_SOCK");
    }
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    command
}

#[derive(Serialize, Deserialize)]
struct Intent {
    binding: RepositoryPublicationBinding,
    repository: crate::catalog::WorkingDirectoryRepository,
}

fn digest(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// Exclusive file creation is also a cross-process no-second-dispatch fence.
// Partial/corrupt records fail closed; they are not treated as absent.
fn persist_new(path: &Path, value: &impl Serialize) -> Result<(), WorkingDirectoryDiagnostic> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| denied("publication evidence encoding failed"))?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| denied("publication evidence could not be created"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .and_then(|_| fs::File::open(path.parent().unwrap())?.sync_all())
        .map_err(|_| denied("publication evidence durability is unknown"))
}

fn read_publication_record(path: &Path) -> Result<Option<Vec<u8>>, WorkingDirectoryDiagnostic> {
    const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= MAX_RECORD_BYTES => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        _ => return Err(denied("publication record is invalid or unavailable")),
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| denied("publication record could not be read"))?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(denied("publication record exceeds its storage bound"));
    }
    Ok(Some(bytes))
}

impl RuntimeGitMaterializer {
    fn publication_root(&self, request: &Request) -> PathBuf {
        self.publication_binding_root(&request.binding)
    }

    fn publication_binding_root(&self, binding: &RepositoryPublicationBinding) -> PathBuf {
        self.runtime_root
            .join(".publications")
            .join(digest(&[&binding.workspace_id, &binding.operation_id]))
    }

    fn publication_request(
        request: &Request,
    ) -> Result<WorkingDirectoryRequest, WorkingDirectoryDiagnostic> {
        let b = &request.binding;
        for field in [
            &b.workspace_id,
            &b.operation_id,
            &b.repository_id,
            &b.repository_source_fingerprint,
            &b.merge_request_id,
            &b.approval_event_id,
            &b.owner_operation_id,
            &b.connection_id,
            &b.worker.runtime_id,
            &b.worker.worker_id,
        ] {
            if field.is_empty() || field.len() > 4096 || field.contains('\0') {
                return Err(denied("publication binding is invalid"));
            }
        }
        if request.repository.id != b.repository_id
            || request.repository.source_fingerprint != b.repository_source_fingerprint
            || request.materialization.workspace_id != b.workspace_id
            || request.materialization.operation_id != b.operation_id
            || request.materialization.runtime_id != b.worker.runtime_id
            || request.materialization.config_projection_digest.is_empty()
        {
            return Err(denied(
                "publication repository or materialization authority mismatch",
            ));
        }
        validate_exact_branch_selector(&b.target)?;
        validate_working_directory_id(&b.working_directory_id)?;
        for oid in [&b.source, &b.target_before] {
            if !matches!(oid.len(), 40 | 64)
                || !oid
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err(denied("publication requires canonical fixed object IDs"));
            }
        }
        if b.source.len() != b.target_before.len() {
            return Err(denied("publication object formats differ"));
        }
        let working = WorkingDirectoryRequest {
            repository: request.repository.clone(),
            display_name: None,
            materializer: MaterializerKind::RuntimeGitClone,
            backend_workdir_id: None,
            materialization: Some(request.materialization.clone()),
        };
        Ok(working)
    }

    pub(super) fn saved_publication(
        &self,
        request: &Request,
    ) -> Result<Option<Receipt>, WorkingDirectoryDiagnostic> {
        Self::publication_request(request)?;
        self.read_publication_receipt(&request.binding, Some(&request.repository))
    }

    pub(super) fn saved_publication_receipt(
        &self,
        binding: &RepositoryPublicationBinding,
    ) -> Result<Option<Receipt>, WorkingDirectoryDiagnostic> {
        // Lookup must not run even check-ref-format: current Git/provider and
        // materialization authority are deliberately absent from this boundary.
        self.read_publication_receipt(binding, None)
    }

    fn read_publication_receipt(
        &self,
        binding: &RepositoryPublicationBinding,
        expected_repository: Option<&crate::catalog::WorkingDirectoryRepository>,
    ) -> Result<Option<Receipt>, WorkingDirectoryDiagnostic> {
        for field in [&binding.workspace_id, &binding.operation_id] {
            if field.is_empty() || field.len() > 4096 || field.chars().any(char::is_control) {
                return Err(denied("publication lookup identity is invalid"));
            }
        }
        let root = self.publication_binding_root(binding);
        // NotFound is absence only after the Runtime-owned storage namespace
        // has been inspected. Permission/I/O errors, non-directories and broken
        // symlinks must never turn retained evidence into an absent operation.
        for directory in [
            &self.runtime_root,
            &self.runtime_root.join(".publications"),
            &root,
        ] {
            match fs::symlink_metadata(directory) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && directory != &self.runtime_root =>
                {
                    return Ok(None);
                }
                _ => return Err(denied("publication evidence namespace is unavailable")),
            }
        }
        let raw = read_publication_record(&root.join("intent.json"))?
            .ok_or_else(|| denied("publication intent is incomplete; do not redispatch"))?;
        let intent: Intent = serde_json::from_slice(&raw)
            .map_err(|_| denied("publication intent is invalid; do not redispatch"))?;
        if &intent.binding != binding
            || intent.repository.id != binding.repository_id
            || intent.repository.source_fingerprint != binding.repository_source_fingerprint
            || expected_repository.is_some_and(|repository| repository != &intent.repository)
        {
            return Err(denied("publication operation fingerprint mismatch"));
        }
        match read_publication_record(&root.join("receipt.json"))? {
            Some(raw) => {
                let receipt: Receipt = serde_json::from_slice(&raw)
                    .map_err(|_| denied("publication receipt is invalid"))?;
                if &receipt.binding != binding
                    || matches!(&receipt.outcome, Outcome::Published { target_after, .. } if target_after != &binding.source)
                {
                    return Err(denied("publication receipt binding mismatch"));
                }
                Ok(Some(receipt))
            }
            None => Ok(Some(Receipt {
                binding: binding.clone(),
                outcome: Outcome::Unknown {},
            })),
        }
    }

    fn finish_publication(
        &self,
        request: &Request,
        outcome: Outcome,
    ) -> Result<Receipt, WorkingDirectoryDiagnostic> {
        let receipt = Receipt {
            binding: request.binding.clone(),
            outcome,
        };
        persist_new(
            &self.publication_root(request).join("receipt.json"),
            &receipt,
        )?;
        Ok(receipt)
    }

    pub(super) fn record_unpublished(
        &self,
        request: &Request,
        reason: &str,
    ) -> Result<Receipt, WorkingDirectoryDiagnostic> {
        if let Some(saved) = self.saved_publication(request)? {
            return Ok(saved);
        }
        let root = self.publication_root(request);
        fs::create_dir_all(root.parent().unwrap())
            .map_err(|_| denied("publication evidence root unavailable"))?;
        if fs::create_dir(&root).is_err() {
            return self
                .saved_publication(request)?
                .ok_or_else(|| denied("publication already reserved"));
        }
        // Persist both directory links before intent can authorize dispatch.
        fs::File::open(root.parent().unwrap())
            .and_then(|f| f.sync_all())
            .and_then(|_| fs::File::open(&self.runtime_root)?.sync_all())
            .map_err(|_| denied("publication evidence directory durability is unknown"))?;
        persist_new(
            &root.join("intent.json"),
            &Intent {
                binding: request.binding.clone(),
                repository: request.repository.clone(),
            },
        )?;
        self.finish_publication(
            request,
            Outcome::NotPublished {
                reason: reason.chars().take(1024).collect(),
            },
        )
    }

    pub(super) fn publish_fixed_source(
        &self,
        request: &Request,
    ) -> Result<Receipt, WorkingDirectoryDiagnostic> {
        let working = Self::publication_request(request)?;
        if let Some(saved) = self.saved_publication(request)? {
            return Ok(saved);
        }
        let root = self.publication_root(request);
        fs::create_dir_all(root.parent().unwrap())
            .map_err(|_| denied("publication evidence root unavailable"))?;
        if fs::create_dir(&root).is_err() {
            return self
                .saved_publication(request)?
                .ok_or_else(|| denied("publication already reserved"));
        }
        // Persist both directory links before intent can authorize dispatch.
        fs::File::open(root.parent().unwrap())
            .and_then(|f| f.sync_all())
            .and_then(|_| fs::File::open(&self.runtime_root)?.sync_all())
            .map_err(|_| denied("publication evidence directory durability is unknown"))?;
        persist_new(
            &root.join("intent.json"),
            &Intent {
                binding: request.binding.clone(),
                repository: request.repository.clone(),
            },
        )?;
        // Everything before this closure returns is known not to have sent push.
        let prepared = self.prepare_publication(request, &working, &root);
        let (mut command, _access, _occupancy) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                return self.finish_publication(
                    request,
                    Outcome::NotPublished {
                        reason: error.message,
                    },
                );
            }
        };
        // Persist the no-retry boundary before any provider receive-pack dispatch.
        persist_new(&root.join("dispatch.json"), &request.binding)?;
        command.args([
            "push",
            "--porcelain",
            "--no-verify",
            "--no-follow-tags",
            "--recurse-submodules=no",
            "--",
            &request.repository.source.uri,
            &format!("{}:{}", request.binding.source, request.binding.target),
        ]);
        let result = run_repository_git_stdout(command, request.repository.source.kind);
        let expected = format!("{}:{}", request.binding.source, request.binding.target);
        if result.as_ref().is_ok_and(|output| {
            output.lines().any(|line| {
                let mut fields = line.split('\t');
                let flag = fields.next();
                let refspec = fields.next();
                let summary = fields.next();
                refspec == Some(expected.as_str())
                    && match flag {
                        Some(" ") => {
                            summary
                                == Some(
                                    format!(
                                        "{}..{}",
                                        request.binding.target_before, request.binding.source
                                    )
                                    .as_str(),
                                )
                        }
                        Some("=") => request.binding.target_before == request.binding.source,
                        _ => false,
                    }
            })
        }) {
            // The successful receive-pack response itself confirms the exact update.
            // Do not discard it if the target moves before a subsequent ls-remote.
            return self.finish_publication(
                request,
                Outcome::Published {
                    target_after: request.binding.source.clone(),
                    observed_at_epoch_seconds: repository_access_now_epoch_seconds(),
                },
            );
        }
        // A nonzero exit/timeout after spawn is not evidence of no external effect.
        Ok(Receipt {
            binding: request.binding.clone(),
            outcome: Outcome::Unknown {},
        })
    }

    fn prepare_publication(
        &self,
        request: &Request,
        working: &WorkingDirectoryRequest,
        root: &Path,
    ) -> Result<
        (Command, Option<RepositoryCommandAccess>, occupancy::Lease),
        WorkingDirectoryDiagnostic,
    > {
        Self::validate_request(working)?;
        if let Some(ssh) = working
            .materialization
            .as_ref()
            .and_then(|m| m.ssh.as_ref())
        {
            validate_repository_access_binding(
                ssh,
                &request.binding.repository_id,
                &request.binding.repository_source_fingerprint,
                Some(&request.repository.source.uri),
            )?;
            if ssh.access != server_api::RepositoryAccessMode::ReadWrite {
                return Err(denied(
                    "publication requires current Repository write access",
                ));
            }
        }
        let occupancy = self
            .occupancy
            .acquire_use(&request.binding.working_directory_id)?;
        let binding = self.checked_binding(&request.binding.working_directory_id, None)?;
        if binding.working_directory.repository_id != request.binding.repository_id
            || binding
                .working_directory
                .evidence
                .repository_source_fingerprint
                .as_ref()
                != Some(&request.binding.repository_source_fingerprint)
            || !binding_paths_are_available(&binding)
        {
            return Err(denied(
                "publication Workdir materialization does not match Repository",
            ));
        }
        let local = |args: &[&str]| -> Result<String, WorkingDirectoryDiagnostic> {
            let mut command = publication_git_command(working, None);
            command
                .current_dir(binding.root())
                .env("GIT_NO_REPLACE_OBJECTS", "1")
                .args([
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args);
            run_repository_git_stdout(command, working.repository.source.kind)
        };
        let git_dir = fs::symlink_metadata(binding.root().join(".git"))
            .map_err(|_| denied("publication Git directory unavailable"))?;
        if !git_dir.is_dir() || git_dir.file_type().is_symlink() {
            return Err(denied(
                "publication Git directory differs from materialized checkout",
            ));
        }
        let top = local(&["rev-parse", "--path-format=absolute", "--show-toplevel"])?;
        if fs::canonicalize(top.trim()).ok() != fs::canonicalize(binding.root()).ok() {
            return Err(denied(
                "publication Git root differs from materialized checkout",
            ));
        }
        if local(&["rev-parse", "--verify", "HEAD^{commit}"])?.trim() != request.binding.source {
            return Err(denied(
                "publication source HEAD changed from approved fixed commit",
            ));
        }
        if !local(&[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ])?
        .trim()
        .is_empty()
        {
            return Err(denied("publication source Workdir is not clean"));
        }
        // Copy approved objects into a Runtime-owned Git directory. Checkout-local
        // URL rewrites, push hooks and push defaults cannot redirect this command.
        let staging = root.join("objects.git");
        let mut init = publication_git_command(working, None);
        init.args([
            "init",
            "--bare",
            "--template=",
            if request.binding.source.len() == 64 {
                "--object-format=sha256"
            } else {
                "--object-format=sha1"
            },
        ])
        .arg(&staging);
        run_repository_git_stdout(init, working.repository.source.kind)?;
        let mut fetch = publication_git_command(working, None);
        fetch
            .arg("-C")
            .arg(&staging)
            .args([
                "-c",
                "protocol.file.allow=always",
                "fetch",
                "--no-tags",
                "--no-write-fetch-head",
                "--",
            ])
            .arg(binding.root())
            .arg(&request.binding.source);
        run_repository_git_stdout(fetch, working.repository.source.kind)?;
        let mut count = publication_git_command(working, None);
        count.arg("-C").arg(&staging).args(["count-objects", "-v"]);
        validate_repository_limits_report(&run_repository_git_stdout(
            count,
            working.repository.source.kind,
        )?)?;
        let mut ancestor = publication_git_command(working, None);
        ancestor.arg("-C").arg(&staging).args([
            "merge-base",
            "--is-ancestor",
            &request.binding.target_before,
            &request.binding.source,
        ]);
        run_repository_git_stdout(ancestor, working.repository.source.kind)
            .map_err(|_| denied("publication is not a fast-forward from expected target"))?;
        let latest = self.observe_publication_target(request, working)?;
        if latest != request.binding.target_before {
            return Err(denied("publication target changed from expected before"));
        }
        // Revalidate expiration immediately before handing the command to dispatch.
        if let Some(ssh) = working
            .materialization
            .as_ref()
            .and_then(|m| m.ssh.as_ref())
        {
            validate_ssh_materialization_access(ssh)?;
        }
        let access = working
            .materialization
            .as_ref()
            .and_then(|m| m.ssh.as_ref())
            .map(|ssh| {
                RepositoryCommandAccess::prepare_fixed_ssh(
                    &self.runtime_root,
                    &request.binding.operation_id,
                    &request.binding.repository_id,
                    ssh,
                    true,
                )
            })
            .transpose()?;
        let mut command = publication_git_command(working, access.as_ref());
        command
            .arg("-C")
            .arg(&staging)
            .args([
                "-c",
                &format!("core.abbrev={}", request.binding.source.len()),
            ])
            .env("GIT_NO_REPLACE_OBJECTS", "1");
        Ok((command, access, occupancy))
    }

    fn observe_publication_target(
        &self,
        request: &Request,
        working: &WorkingDirectoryRequest,
    ) -> Result<String, WorkingDirectoryDiagnostic> {
        Self::validate_request(working)?;
        let staging = self.publication_root(request).join("objects.git");
        if !staging.is_dir() {
            return Err(denied("publication object snapshot is unavailable"));
        }
        // Reconciliation can run while send is in flight. Its temporary SSH
        // resources must not replace/remove the sender's agent or wrapper.
        let access = working
            .materialization
            .as_ref()
            .and_then(|m| m.ssh.as_ref())
            .map(|ssh| {
                RepositoryCommandAccess::prepare_fixed_ssh(
                    &self.runtime_root,
                    &format!("publication-observation-{}", uuid::Uuid::now_v7()),
                    &request.binding.repository_id,
                    ssh,
                    false,
                )
            })
            .transpose()?;
        let mut command = publication_git_command(working, access.as_ref());
        command.arg("-C").arg(&staging).args([
            "ls-remote",
            "--exit-code",
            "--refs",
            "--",
            &request.repository.source.uri,
            &request.binding.target,
        ]);
        let report = run_repository_git_stdout(command, working.repository.source.kind)?;
        let mut lines = report.lines();
        let (oid, selector) = lines
            .next()
            .and_then(|line| line.split_once('\t'))
            .ok_or_else(|| denied("publication target observation invalid"))?;
        if lines.next().is_some()
            || selector != request.binding.target
            || oid.len() != request.binding.source.len()
            || !oid.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(denied("publication target observation invalid"));
        }
        Ok(oid.to_ascii_lowercase())
    }

    pub(super) fn reconcile_fixed_source(
        &self,
        request: &Request,
    ) -> Result<Receipt, WorkingDirectoryDiagnostic> {
        let saved = self
            .saved_publication(request)?
            .ok_or_else(|| denied("publication operation was not found"))?;
        if !matches!(saved.outcome, Outcome::Unknown {}) {
            return Ok(saved);
        }
        // Observation is useful diagnostic evidence, never proof that this
        // operation (rather than another publisher) performed the update.
        let working = Self::publication_request(request)?;
        let _observation = self.observe_publication_target(request, &working);
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    //! Real Git/bare-provider and Runtime-owned receipt storage; only crash windows
    //! are injected by retaining the same intent without a receive-pack receipt.
    use super::super::tests::{create_clean_repo, git, request};
    use super::*;
    use crate::catalog::RepositoryMaterializationContext;
    use crate::identity::{RuntimeWorkerRef, WorkerId};

    struct Fixture {
        _source: tempfile::TempDir,
        remote: tempfile::TempDir,
        runtime: tempfile::TempDir,
        materializer: RuntimeGitMaterializer,
        checkout: PathBuf,
        request: Request,
    }
    fn oid(path: &Path) -> String {
        git_stdout(path, ["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string()
    }
    fn commit(path: &Path, text: &str) {
        fs::write(path.join("README.md"), text).unwrap();
        git(
            path,
            &[
                "-c",
                "user.name=Publication Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-am",
                text,
            ],
        );
    }
    impl Fixture {
        fn new() -> Self {
            let source = create_clean_repo();
            git(source.path(), &["branch", "-M", "main"]);
            let before = oid(source.path());
            let remote = tempfile::tempdir().unwrap();
            git(
                remote.path(),
                &["clone", "--bare", source.path().to_str().unwrap(), "."],
            );
            let runtime = tempfile::tempdir().unwrap();
            let materializer = RuntimeGitMaterializer::new(runtime.path());
            let working = request(remote.path());
            let binding = materializer.create(&working).unwrap();
            let checkout = binding.root().to_path_buf();
            commit(&checkout, "approved");
            let source_oid = oid(&checkout);
            let request = Request {
                binding: RepositoryPublicationBinding {
                    workspace_id: "workspace".into(),
                    merge_request_id: "mr".into(),
                    operation_id: "publication".into(),
                    repository_id: working.repository.id.clone(),
                    repository_source_fingerprint: working.repository.source_fingerprint.clone(),
                    target: "refs/heads/main".into(),
                    source: source_oid,
                    approval_event_id: "approval".into(),
                    owner_operation_id: "owner".into(),
                    worker: RuntimeWorkerRef::new("runtime", WorkerId::now_v7().to_string()),
                    working_directory_id: binding.working_directory.id.clone(),
                    connection_id: "connection".into(),
                    alias: workdir::WorkdirAttachmentAlias::new("source").unwrap(),
                    target_before: before,
                },
                repository: working.repository,
                materialization: RepositoryMaterializationContext {
                    workspace_id: "workspace".into(),
                    runtime_id: "runtime".into(),
                    operation_id: "publication".into(),
                    config_projection_digest: "current-config".into(),
                    ssh: None,
                },
            };
            Self {
                _source: source,
                remote,
                runtime,
                materializer,
                checkout,
                request,
            }
        }
        fn assert_not_published(&self) {
            let receipt = self
                .materializer
                .publish_fixed_source(&self.request)
                .unwrap();
            assert!(
                matches!(receipt.outcome, Outcome::NotPublished { .. }),
                "{receipt:?}"
            );
            assert_eq!(oid(self.remote.path()), self.request.binding.target_before);
            assert!(
                !self
                    .materializer
                    .publication_root(&self.request)
                    .join("dispatch.json")
                    .exists()
            );
            assert_eq!(
                self.materializer
                    .reconcile_fixed_source(&self.request)
                    .unwrap(),
                receipt
            );
        }
    }

    #[test]
    fn fixed_source_ff_receipt_survives_target_movement_restart_and_replay() {
        let fixture = Fixture::new();
        let receipt = fixture
            .materializer
            .publish_fixed_source(&fixture.request)
            .unwrap();
        assert!(
            matches!(receipt.outcome, Outcome::Published { .. }),
            "{receipt:?}"
        );
        assert_eq!(oid(fixture.remote.path()), fixture.request.binding.source);
        commit(&fixture.checkout, "later");
        git(
            &fixture.checkout,
            &[
                "push",
                fixture.remote.path().to_str().unwrap(),
                "HEAD:refs/heads/main",
            ],
        );
        let later = oid(fixture.remote.path());
        let restarted = RuntimeGitMaterializer::new(fixture.runtime.path());
        assert_eq!(
            restarted.publish_fixed_source(&fixture.request).unwrap(),
            receipt
        );
        assert_eq!(
            restarted.reconcile_fixed_source(&fixture.request).unwrap(),
            receipt
        );
        assert_eq!(oid(fixture.remote.path()), later);
    }

    #[test]
    fn changed_head_and_dirty_workdir_never_dispatch() {
        let fixture = Fixture::new();
        commit(&fixture.checkout, "unapproved");
        fixture.assert_not_published();
        let fixture = Fixture::new();
        fs::write(fixture.checkout.join("untracked"), "dirty").unwrap();
        fixture.assert_not_published();
    }

    #[test]
    fn non_ff_target_and_stale_target_never_dispatch() {
        let mut fixture = Fixture::new();
        git(
            &fixture.checkout,
            &[
                "checkout",
                "--detach",
                &fixture.request.binding.target_before,
            ],
        );
        commit(&fixture.checkout, "divergent");
        let divergent = oid(&fixture.checkout);
        git(
            &fixture.checkout,
            &[
                "push",
                fixture.remote.path().to_str().unwrap(),
                "HEAD:refs/heads/main",
            ],
        );
        git(
            &fixture.checkout,
            &["checkout", "--detach", &fixture.request.binding.source],
        );
        fixture.request.binding.target_before = divergent;
        fixture.assert_not_published();
        let fixture = Fixture::new();
        let mut changed = fixture.request.clone();
        changed.binding.target_before = "a".repeat(40);
        assert!(matches!(
            fixture
                .materializer
                .publish_fixed_source(&changed)
                .unwrap()
                .outcome,
            Outcome::NotPublished { .. }
        ));
        assert_eq!(
            oid(fixture.remote.path()),
            fixture.request.binding.target_before
        );
    }

    #[test]
    fn unresolved_dispatch_replay_and_recovery_never_push_or_infer_success() {
        let fixture = Fixture::new();
        let root = fixture.materializer.publication_root(&fixture.request);
        fs::create_dir_all(&root).unwrap();
        persist_new(
            &root.join("intent.json"),
            &Intent {
                binding: fixture.request.binding.clone(),
                repository: fixture.request.repository.clone(),
            },
        )
        .unwrap();
        persist_new(&root.join("dispatch.json"), &fixture.request.binding).unwrap();
        let restarted = RuntimeGitMaterializer::new(fixture.runtime.path());
        assert!(matches!(
            restarted
                .publish_fixed_source(&fixture.request)
                .unwrap()
                .outcome,
            Outcome::Unknown {}
        ));
        assert_eq!(
            oid(fixture.remote.path()),
            fixture.request.binding.target_before
        );
        git(
            &fixture.checkout,
            &[
                "push",
                fixture.remote.path().to_str().unwrap(),
                "HEAD:refs/heads/main",
            ],
        );
        assert!(matches!(
            restarted
                .reconcile_fixed_source(&fixture.request)
                .unwrap()
                .outcome,
            Outcome::Unknown {}
        ));
        assert!(!root.join("receipt.json").exists());
    }

    #[test]
    fn binding_and_repository_and_workspace_mismatches_fail_closed() {
        let fixture = Fixture::new();
        let receipt = fixture
            .materializer
            .publish_fixed_source(&fixture.request)
            .unwrap();
        let mut changed = fixture.request.clone();
        changed.binding.connection_id = "replacement".into();
        assert!(fixture.materializer.publish_fixed_source(&changed).is_err());
        changed = fixture.request.clone();
        changed.repository.source.uri = "/wrong/repository".into();
        assert!(
            fixture
                .materializer
                .reconcile_fixed_source(&changed)
                .is_err()
        );
        changed = fixture.request.clone();
        changed.materialization.workspace_id = "other".into();
        assert!(
            fixture
                .materializer
                .reconcile_fixed_source(&changed)
                .is_err()
        );
        assert_eq!(
            fixture
                .materializer
                .reconcile_fixed_source(&fixture.request)
                .unwrap(),
            receipt
        );
    }

    #[test]
    fn checkout_push_configuration_cannot_redirect_publication() {
        let fixture = Fixture::new();
        git(
            &fixture.checkout,
            &[
                "config",
                "url./does-not-exist/.pushInsteadOf",
                fixture.remote.path().to_str().unwrap(),
            ],
        );
        git(
            &fixture.checkout,
            &["config", "core.hooksPath", "/does-not-exist"],
        );
        let receipt = fixture
            .materializer
            .publish_fixed_source(&fixture.request)
            .unwrap();
        assert!(
            matches!(receipt.outcome, Outcome::Published { .. }),
            "{receipt:?}"
        );
        assert_eq!(oid(fixture.remote.path()), fixture.request.binding.source);
    }

    #[test]
    fn provider_reject_after_dispatch_remains_unknown() {
        let fixture = Fixture::new();
        // A real receive-pack rejection is intentionally not treated as proof
        // of no effect; the same classification covers a lost acknowledgement.
        git(
            fixture.remote.path(),
            &["config", "receive.denyCurrentBranch", "refuse"],
        );
        git(fixture.remote.path(), &["config", "core.bare", "false"]);
        let receipt = fixture
            .materializer
            .publish_fixed_source(&fixture.request)
            .unwrap();
        assert!(
            matches!(receipt.outcome, Outcome::Unknown {}),
            "{receipt:?}"
        );
        assert!(
            fixture
                .materializer
                .publication_root(&fixture.request)
                .join("dispatch.json")
                .exists()
        );
        git(fixture.remote.path(), &["config", "core.bare", "true"]);
        assert!(matches!(
            fixture
                .materializer
                .publish_fixed_source(&fixture.request)
                .unwrap()
                .outcome,
            Outcome::Unknown {}
        ));
        assert_eq!(
            oid(fixture.remote.path()),
            fixture.request.binding.target_before
        );
    }
    #[test]
    fn pre_dispatch_resource_rejection_is_durable_and_cannot_be_retried_as_push() {
        let fixture = Fixture::new();
        let receipt = fixture
            .materializer
            .record_unpublished(&fixture.request, "Workdir conflict")
            .unwrap();
        assert!(matches!(receipt.outcome, Outcome::NotPublished { .. }));
        let restarted = RuntimeGitMaterializer::new(fixture.runtime.path());
        assert_eq!(
            restarted.publish_fixed_source(&fixture.request).unwrap(),
            receipt
        );
        assert_eq!(
            oid(fixture.remote.path()),
            fixture.request.binding.target_before
        );
    }
    #[test]
    fn binding_only_receipt_lookup_needs_no_provider_checkout_or_credentials() {
        let fixture = Fixture::new();
        let receipt = fixture
            .materializer
            .publish_fixed_source(&fixture.request)
            .unwrap();
        let binding = fixture.request.binding.clone();
        drop(fixture.request);
        fixture.remote.close().unwrap();
        fs::remove_dir_all(&fixture.checkout).unwrap();
        let restarted = RuntimeGitMaterializer::new(fixture.runtime.path());
        let calls = test_repository_git_invocation_count();
        assert_eq!(
            restarted.saved_publication_receipt(&binding).unwrap(),
            Some(receipt)
        );
        assert_eq!(
            test_repository_git_invocation_count(),
            calls,
            "lookup must not invoke Git or contact the provider"
        );
    }

    #[test]
    fn binding_only_receipt_lookup_distinguishes_absence_unknown_and_corruption() {
        let fixture = Fixture::new();
        let b = &fixture.request.binding;
        let root = fixture.materializer.publication_binding_root(b);
        let calls = test_repository_git_invocation_count();
        assert_eq!(
            fixture.materializer.saved_publication_receipt(b).unwrap(),
            None
        );
        fs::create_dir_all(&root).unwrap();
        assert!(
            fixture.materializer.saved_publication_receipt(b).is_err(),
            "an incomplete reservation is not absence"
        );
        fs::write(root.join("intent.json"), b"{").unwrap();
        assert!(fixture.materializer.saved_publication_receipt(b).is_err());
        fs::write(
            root.join("intent.json"),
            serde_json::to_vec(&Intent {
                binding: b.clone(),
                repository: fixture.request.repository.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            fixture
                .materializer
                .saved_publication_receipt(b)
                .unwrap()
                .unwrap()
                .outcome,
            Outcome::Unknown {}
        ));
        let mut wrong = b.clone();
        wrong.target = "refs/heads/other".into();
        assert!(
            fixture
                .materializer
                .saved_publication_receipt(&wrong)
                .is_err()
        );
        wrong = b.clone();
        wrong.worker.worker_id = WorkerId::now_v7().to_string();
        assert!(
            fixture
                .materializer
                .saved_publication_receipt(&wrong)
                .is_err()
        );
        wrong = b.clone();
        wrong.repository_source_fingerprint = "changed-source".into();
        assert!(
            fixture
                .materializer
                .saved_publication_receipt(&wrong)
                .is_err()
        );
        fs::write(root.join("receipt.json"), b"{}").unwrap();
        assert!(fixture.materializer.saved_publication_receipt(b).is_err());
        fs::write(
            root.join("receipt.json"),
            serde_json::to_vec(&Receipt {
                binding: b.clone(),
                outcome: Outcome::Published {
                    target_after: "0".repeat(40),
                    observed_at_epoch_seconds: 1,
                },
            })
            .unwrap(),
        )
        .unwrap();
        assert!(fixture.materializer.saved_publication_receipt(b).is_err());
        assert_eq!(test_repository_git_invocation_count(), calls);
    }

    #[cfg(unix)]
    #[test]
    fn binding_only_receipt_lookup_rejects_unavailable_or_symlinked_namespace() {
        let fixture = Fixture::new();
        let binding = &fixture.request.binding;
        let namespace = fixture.runtime.path().join(".publications");
        fs::write(&namespace, "not a directory").unwrap();
        assert!(
            fixture
                .materializer
                .saved_publication_receipt(binding)
                .is_err()
        );
        fs::remove_file(&namespace).unwrap();
        fs::create_dir(&namespace).unwrap();
        let operation = fixture.materializer.publication_binding_root(binding);
        std::os::unix::fs::symlink(namespace.join("missing"), &operation).unwrap();
        assert!(
            fixture
                .materializer
                .saved_publication_receipt(binding)
                .is_err(),
            "broken symlinks are not absent operations"
        );
    }
}
