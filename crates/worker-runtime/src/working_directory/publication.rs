//! Each publication runs current safety checks and a single non-force push.
//! Results are ephemeral: neither old Runtime files nor ref equality authorize
//! recovery or another push. The caller decides what to do after an unknown result.
use super::*;
use crate::catalog::{
    RepositoryPublicationBinding, RepositoryPublicationOutcome as Outcome,
    RepositoryPublicationRequest as Request, RepositoryPublicationResult as PublicationResult,
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

// The isolated object snapshot lives only for this execution. Leftovers after
// process loss are never read as intent, evidence, or permission for another push.
struct PublicationScratch(PathBuf);

impl Drop for PublicationScratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Only a complete single-ref porcelain response can settle a dispatched operation.
// Remote failure is not rejection: the provider may have updated the ref before
// losing its acknowledgement. Abbreviated OIDs cannot establish the exact before.
fn publication_outcome(
    binding: &RepositoryPublicationBinding,
    output: &RepositoryGitOutput,
) -> Option<Outcome> {
    if output.status.code().is_none() || !output.stdout.complete {
        return None;
    }
    let stdout = std::str::from_utf8(&output.stdout.bytes).ok()?;
    if !stdout.ends_with('\n') {
        return None;
    }
    let mut lines = stdout.lines();
    if lines.next()?.strip_prefix("To ")?.is_empty() {
        return None;
    }
    let mut fields = lines.next()?.split('\t');
    let flag = fields.next()?;
    let refspec = fields.next()?;
    let summary = fields.next()?;
    if fields.next().is_some()
        || refspec != format!("{}:{}", binding.source, binding.target)
        || lines.next() != Some("Done")
        || lines.next().is_some()
    {
        return None;
    }
    match flag {
        " " if output.status.success()
            && summary == format!("{}..{}", binding.target_before, binding.source) => {}
        "=" if output.status.success()
            && binding.target_before == binding.source
            && summary == "[up to date]" => {}
        "!" if !output.status.success() => {
            // Git distinguishes an explicit local/remote rejection from a remote
            // failure (including missing report-status), even though all use '!'.
            let reason = summary
                .strip_prefix("[rejected] (")
                .or_else(|| summary.strip_prefix("[remote rejected] ("))?
                .strip_suffix(')')?;
            if reason.is_empty() || reason.chars().any(char::is_control) {
                return None;
            }
            return Some(Outcome::NotPublished {
                reason: "Git rejected the exact publication ref".into(),
            });
        }
        _ => return None,
    }
    Some(Outcome::Published {
        target_after: binding.source.clone(),
        observed_at_epoch_seconds: repository_access_now_epoch_seconds(),
    })
}

impl RuntimeGitMaterializer {
    pub(super) fn verify_provider_ancestor(
        &self,
        working: &WorkingDirectoryRequest,
        access: Option<&RepositoryCommandAccess>,
        selector: &str,
        target: &str,
        ancestor: &str,
    ) -> Result<bool, WorkingDirectoryDiagnostic> {
        if ancestor.len() != target.len()
            || !ancestor
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(WorkingDirectoryDiagnostic::new(
                WorkdirManagementReason::RefResponseInvalid,
                "Repository containment requires a canonical commit ID",
            ));
        }
        let root = self
            .runtime_root
            .join(format!(".ref-observation-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&self.runtime_root)
            .map_err(|_| denied("observation directory unavailable"))?;
        fs::create_dir(&root).map_err(|_| denied("observation scratch directory unavailable"))?;
        let scratch = PublicationScratch(root);
        set_directory_owner_only(&scratch.0)?;
        let command = |args: &[&str]| {
            let mut command = publication_git_command(working, access);
            command.arg("-C").arg(&scratch.0).args(args);
            run_repository_git_stdout(command, working.repository.source.kind)
        };
        command(&[
            "init",
            "--bare",
            "--template=",
            if target.len() == 64 {
                "--object-format=sha256"
            } else {
                "--object-format=sha1"
            },
        ])?;
        // Fetch only from the configured provider into an isolated object database;
        // neither a Coder checkout nor replace refs can supply the ancestry proof.
        command(&[
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            "--",
            &working.repository.source.uri,
            &format!("{selector}:refs/observation/target"),
        ])?;
        let fetched = command(&["rev-parse", "--verify", "refs/observation/target^{commit}"])?;
        if fetched.trim() != target {
            return Err(WorkingDirectoryDiagnostic::new(
                WorkdirManagementReason::RefResponseInvalid,
                "Repository target changed during containment observation",
            ));
        }
        let mut ancestry = publication_git_command(working, access);
        ancestry
            .arg("-C")
            .arg(&scratch.0)
            .args(["merge-base", "--is-ancestor", ancestor, target]);
        let output = run_repository_git_output(ancestry, REPOSITORY_COMMAND_TIMEOUT)?;
        match output.status.code() {
            Some(0) => Ok(true),
            // An unrelated or absent commit supplies no containment proof. The
            // caller must not turn this observation into a successful completion.
            Some(1 | 128) => Ok(false),
            _ => Err(WorkingDirectoryDiagnostic::new(
                WorkdirManagementReason::RefProviderUnavailable,
                "Repository containment check could not be completed",
            )),
        }
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

    pub(super) fn publish_fixed_source(
        &self,
        request: &Request,
    ) -> Result<PublicationResult, WorkingDirectoryDiagnostic> {
        let working = Self::publication_request(request)?;
        let root = self
            .runtime_root
            .join(format!(".publication-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&root).map_err(|_| denied("publication scratch directory unavailable"))?;
        let scratch = PublicationScratch(root);
        set_directory_owner_only(&scratch.0)?;
        // Everything before preparation returns is known not to have sent push.
        let (mut command, _access, _occupancy) =
            match self.prepare_publication(request, &working, &scratch.0) {
                Ok(prepared) => prepared,
                Err(error) => {
                    return Ok(PublicationResult {
                        binding: request.binding.clone(),
                        outcome: Outcome::NotPublished {
                            reason: error.message,
                        },
                    });
                }
            };
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
        let output = run_repository_git_output(command, REPOSITORY_COMMAND_TIMEOUT);
        // A nonzero exit/timeout after spawn is not evidence of no external effect.
        let outcome = output
            .as_ref()
            .ok()
            .and_then(|output| publication_outcome(&request.binding, output))
            .unwrap_or(Outcome::Unknown {});
        Ok(PublicationResult {
            binding: request.binding.clone(),
            outcome,
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
        let latest = self.observe_publication_target(request, working, &staging)?;
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
                    &format!("publication-send-{}", uuid::Uuid::now_v7()),
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
        staging: &Path,
    ) -> Result<String, WorkingDirectoryDiagnostic> {
        Self::validate_request(working)?;
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
}

#[cfg(test)]
mod tests {
    //! Real Git/bare-provider and isolated snapshots. Provider hooks inject rejected
    //! or missing acknowledgements; no live credentials or external service is used.
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
        fn assert_no_publication_files(&self) {
            assert!(!self.runtime.path().join(".publications").exists());
            assert!(fs::read_dir(self.runtime.path()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".publication-")
            }));
        }
        fn assert_not_published(&self) {
            let result = self
                .materializer
                .publish_fixed_source(&self.request)
                .unwrap();
            assert!(
                matches!(result.outcome, Outcome::NotPublished { .. }),
                "{result:?}"
            );
            assert_eq!(oid(self.remote.path()), self.request.binding.target_before);
            self.assert_no_publication_files();
        }
    }

    #[test]
    fn fixed_source_ff_result_is_ephemeral_and_reexecution_checks_current_source() {
        let fixture = Fixture::new();
        let result = fixture
            .materializer
            .publish_fixed_source(&fixture.request)
            .unwrap();
        assert!(
            matches!(result.outcome, Outcome::Published { .. }),
            "{result:?}"
        );
        assert_eq!(oid(fixture.remote.path()), fixture.request.binding.source);
        fixture.assert_no_publication_files();
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
        let result = restarted.publish_fixed_source(&fixture.request).unwrap();
        assert!(
            matches!(result.outcome, Outcome::NotPublished { .. }),
            "{result:?}"
        );
        assert_eq!(oid(fixture.remote.path()), later);
        fixture.assert_no_publication_files();
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
    fn old_publication_files_neither_authorize_nor_block_new_execution() {
        let fixture = Fixture::new();
        // Populate the former operation-keyed layout, including a forged success.
        let mut hash = Sha256::new();
        for part in [
            &fixture.request.binding.workspace_id,
            &fixture.request.binding.operation_id,
        ] {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part.as_bytes());
        }
        let key = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let root = fixture.runtime.path().join(".publications").join(key);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("intent.json"),
            serde_json::to_vec(&serde_json::json!({
                "binding": fixture.request.binding, "repository": fixture.request.repository,
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("dispatch.json"),
            serde_json::to_vec(&fixture.request.binding).unwrap(),
        )
        .unwrap();
        let old = serde_json::to_vec(&PublicationResult {
            binding: fixture.request.binding.clone(),
            outcome: Outcome::Published {
                target_after: fixture.request.binding.source.clone(),
                observed_at_epoch_seconds: 1,
            },
        })
        .unwrap();
        fs::write(root.join("receipt.json"), &old).unwrap();
        let restarted = RuntimeGitMaterializer::new(fixture.runtime.path());
        fs::write(fixture.checkout.join("untracked"), "dirty").unwrap();
        let rejected = restarted.publish_fixed_source(&fixture.request).unwrap();
        assert!(matches!(rejected.outcome, Outcome::NotPublished { .. }));
        assert_eq!(
            oid(fixture.remote.path()),
            fixture.request.binding.target_before
        );
        fs::remove_file(fixture.checkout.join("untracked")).unwrap();
        // Even corrupt old files have no authority or reservation semantics.
        fs::write(root.join("intent.json"), b"{").unwrap();
        let result = restarted.publish_fixed_source(&fixture.request).unwrap();
        assert!(
            matches!(result.outcome, Outcome::Published { .. }),
            "{result:?}"
        );
        assert_eq!(oid(fixture.remote.path()), fixture.request.binding.source);
        assert_eq!(fs::read(root.join("receipt.json")).unwrap(), old);
        assert_eq!(fs::read(root.join("intent.json")).unwrap(), b"{");
    }

    #[test]
    fn binding_repository_and_workspace_mismatches_fail_closed() {
        let fixture = Fixture::new();
        for case in [
            "repository",
            "fingerprint",
            "workspace",
            "runtime",
            "operation",
        ] {
            let mut changed = fixture.request.clone();
            match case {
                "repository" => changed.repository.id = "wrong".into(),
                "fingerprint" => changed.repository.source_fingerprint = "wrong".into(),
                "workspace" => changed.materialization.workspace_id = "wrong".into(),
                "runtime" => changed.materialization.runtime_id = "wrong".into(),
                "operation" => changed.materialization.operation_id = "wrong".into(),
                _ => unreachable!(),
            }
            assert!(
                fixture.materializer.publish_fixed_source(&changed).is_err(),
                "{case}"
            );
            assert_eq!(
                oid(fixture.remote.path()),
                fixture.request.binding.target_before
            );
        }
        fixture.assert_no_publication_files();
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
        let result = fixture
            .materializer
            .publish_fixed_source(&fixture.request)
            .unwrap();
        assert!(
            matches!(result.outcome, Outcome::Published { .. }),
            "{result:?}"
        );
        assert_eq!(oid(fixture.remote.path()), fixture.request.binding.source);
    }

    #[cfg(unix)]
    #[test]
    fn provider_reject_returns_not_published_without_persistent_reservation() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let hook = fixture.remote.path().join("hooks/pre-receive");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        let result = fixture
            .materializer
            .publish_fixed_source(&fixture.request)
            .unwrap();
        assert!(
            matches!(result.outcome, Outcome::NotPublished { .. }),
            "{result:?}"
        );
        assert_eq!(
            oid(fixture.remote.path()),
            fixture.request.binding.target_before
        );
        fixture.assert_no_publication_files();
        fs::remove_file(hook).unwrap();
        // An explicit caller decision executes fresh checks; no saved rejection is replayed.
        let result = fixture
            .materializer
            .publish_fixed_source(&fixture.request)
            .unwrap();
        assert!(
            matches!(result.outcome, Outcome::Published { .. }),
            "{result:?}"
        );
        assert_eq!(oid(fixture.remote.path()), fixture.request.binding.source);
        fixture.assert_no_publication_files();
    }
    #[cfg(unix)]
    #[test]
    fn porcelain_requires_complete_exact_ref_evidence_not_exit_status() {
        use std::os::unix::process::ExitStatusExt;
        let fixture = Fixture::new();
        let b = &fixture.request.binding;
        let response = |line: &str| format!("To provider\n{line}\nDone\n");
        let rejected = format!(
            "!\t{}:{}\t[remote rejected] (pre-receive hook declined)",
            b.source, b.target
        );
        let local_rejected = format!("!\t{}:{}\t[rejected] (fetch first)", b.source, b.target);
        let success = format!(
            " \t{}:{}\t{}..{}",
            b.source, b.target, b.target_before, b.source
        );
        let output = |code, text: String| RepositoryGitOutput {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: BoundedCommandOutput {
                bytes: text.into_bytes(),
                complete: true,
            },
            stderr: BoundedCommandOutput::default(),
        };
        for line in [&rejected, &local_rejected] {
            assert!(matches!(
                publication_outcome(b, &output(1, response(line))),
                Some(Outcome::NotPublished { .. })
            ));
        }
        assert!(matches!(
            publication_outcome(b, &output(0, response(&success))),
            Some(Outcome::Published { .. })
        ));
        for (code, text) in [
            (1, String::new()),
            (0, String::new()),
            (0, response(&rejected)),
            (1, response(&success)),
            (
                1,
                response(&rejected.replace(&b.target, "refs/heads/other")),
            ),
            (1, response(&rejected.replace(&b.source, &"0".repeat(40)))),
            (
                1,
                response(&rejected.replace("[remote rejected]", "[remote failure]")),
            ),
            (
                1,
                response(&rejected.replace("[remote rejected]", "[unknown]")),
            ),
            (1, response(&rejected).replace("Done\n", "")),
            (1, response(&rejected).trim_end().to_string()),
            (1, response(&format!("{rejected}\n{rejected}"))),
            (1, response(&format!("{rejected}\n{success}"))),
            (1, response(&format!("{rejected}\textra"))),
            (
                0,
                response(&success.replace(
                    &format!("{}..{}", b.target_before, b.source),
                    "1234567..abcdef0",
                )),
            ),
            (
                0,
                response(&success.replace(&b.target_before, &"0".repeat(40))),
            ),
            (
                0,
                response(&format!("=\t{}:{}\t[up to date]", b.source, b.target)),
            ),
        ] {
            assert!(
                publication_outcome(b, &output(code, text.clone())).is_none(),
                "code={code} text={text:?}"
            );
        }
        let mut incomplete = output(1, response(&rejected));
        incomplete.stdout.complete = false;
        assert!(publication_outcome(b, &incomplete).is_none());
        incomplete.stdout.complete = true;
        incomplete.status = std::process::ExitStatus::from_raw(9);
        assert!(publication_outcome(b, &incomplete).is_none());
        incomplete.status = std::process::ExitStatus::from_raw(256);
        incomplete.stdout.bytes.push(0xff);
        assert!(publication_outcome(b, &incomplete).is_none());
        let mut unchanged = b.clone();
        unchanged.target_before = unchanged.source.clone();
        assert!(matches!(
            publication_outcome(
                &unchanged,
                &output(
                    0,
                    response(&format!("=\t{}:{}\t[up to date]", b.source, b.target))
                )
            ),
            Some(Outcome::Published { .. })
        ));
    }

    #[test]
    fn real_git_success_requires_full_oid_summary_not_abbreviations() {
        for abbrev in [7, 40] {
            let fixture = Fixture::new();
            let b = &fixture.request.binding;
            let mut command = isolated_git_command();
            command.current_dir(&fixture.checkout).args([
                "-c",
                &format!("core.abbrev={abbrev}"),
                "push",
                "--porcelain",
                "--",
                fixture.remote.path().to_str().unwrap(),
                &format!("{}:{}", b.source, b.target),
            ]);
            let output = run_repository_git_output(command, REPOSITORY_COMMAND_TIMEOUT).unwrap();
            assert!(output.status.success());
            assert!(output.stdout.complete);
            assert_eq!(oid(fixture.remote.path()), b.source);
            let outcome = publication_outcome(b, &output);
            assert_eq!(
                matches!(outcome, Some(Outcome::Published { .. })),
                abbrev == 40,
                "{}",
                String::from_utf8_lossy(&output.stdout.bytes)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn provider_missing_ack_returns_unknown_without_retry_or_persistent_occupancy() {
        use std::os::unix::fs::PermissionsExt;
        // The same missing acknowledgement can occur before or after an actual update.
        for hook_name in ["pre-receive", "post-receive"] {
            let fixture = Fixture::new();
            let hook = fixture.remote.path().join("hooks").join(hook_name);
            fs::write(&hook, "#!/bin/sh\nkill -KILL \"$PPID\"\nexit 1\n").unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
            let result = fixture
                .materializer
                .publish_fixed_source(&fixture.request)
                .unwrap();
            assert!(
                matches!(result.outcome, Outcome::Unknown {}),
                "{hook_name}: {result:?}"
            );
            let expected = if hook_name == "pre-receive" {
                &fixture.request.binding.target_before
            } else {
                &fixture.request.binding.source
            };
            assert_eq!(&oid(fixture.remote.path()), expected);
            fixture.assert_no_publication_files();
            fs::remove_file(hook).unwrap();
            // A fresh, explicitly requested execution still has to match current target.
            let restarted = RuntimeGitMaterializer::new(fixture.runtime.path());
            let next = restarted.publish_fixed_source(&fixture.request).unwrap();
            assert_eq!(
                matches!(next.outcome, Outcome::Published { .. }),
                hook_name == "pre-receive",
                "{next:?}"
            );
            if hook_name == "post-receive" {
                assert!(matches!(next.outcome, Outcome::NotPublished { .. }));
            }
            assert_eq!(oid(fixture.remote.path()), fixture.request.binding.source);
            fixture.assert_no_publication_files();
        }
    }

    #[cfg(unix)]
    #[test]
    fn git_output_preserves_nonzero_stdout_and_existing_caller_errors() {
        let command = |script: &str| {
            let mut command = Command::new("sh");
            command.args(["-c", script]);
            command
        };
        let output = run_repository_git_output(
            command("printf response; printf diagnostic >&2; exit 1"),
            REPOSITORY_COMMAND_TIMEOUT,
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(output.stdout.bytes, b"response");
        assert!(output.stdout.complete);
        assert_eq!(output.stderr.bytes, b"diagnostic");
        assert!(output.stderr.complete);
        let kind = server_api::RepositorySourceKind::LocalPath;
        assert!(run_repository_git_stdout(command("printf response; exit 1"), kind).is_err());
        assert_eq!(
            run_repository_git_stdout(command("printf response"), kind).unwrap(),
            "response"
        );
        assert_eq!(
            run_repository_git_stdout(command("exit 2"), kind)
                .unwrap_err()
                .code,
            WorkdirManagementReason::RefNotFound.as_str()
        );
        for (kind, expected) in [
            (
                server_api::RepositorySourceKind::Https,
                WorkdirManagementReason::RefProviderAuthFailed,
            ),
            (
                server_api::RepositorySourceKind::LocalPath,
                WorkdirManagementReason::RefProviderUnavailable,
            ),
        ] {
            let error =
                run_repository_git_stdout(command("printf 'permission denied' >&2; exit 1"), kind)
                    .unwrap_err();
            assert_eq!(error.code, expected.as_str());
            assert!(!error.message.contains("permission denied"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn timeout_cannot_supply_completed_porcelain_evidence() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 'To provider\\n!\\tref\\t[remote rejected] (reason)\\nDone\\n'; while :; do :; done"]);
        let error = run_repository_git_output(command, Duration::from_millis(50))
            .err()
            .unwrap();
        assert_eq!(
            error.code,
            WorkdirManagementReason::RefProviderTimeout.as_str()
        );
    }

    #[test]
    fn bounded_capture_distinguishes_eof_truncation_and_read_failure() {
        for size in [8191, 8192, 8193, 16384] {
            let output = capture_bounded_command_output(std::io::Cursor::new(vec![b'x'; size]));
            assert_eq!(output.bytes.len(), size.min(8192));
            assert_eq!(output.complete, size <= 8192);
        }
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected read failure"))
            }
        }
        assert!(!capture_bounded_command_output(Broken).complete);
    }
}
