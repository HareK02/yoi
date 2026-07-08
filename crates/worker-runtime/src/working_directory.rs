use crate::catalog::{
    DirtyStatePolicy, MaterializerKind, WorkingDirectoryCleanupTarget, WorkingDirectoryRequest,
    WorkingDirectoryStatus, WorkingDirectoryStatusKind, WorkingDirectorySummary,
};
use crate::identity::WorkerRef;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const WORKING_DIRECTORIES_DIR: &str = "working-directories";
const MATERIALIZATION_RECORD: &str = "materialization.json";
static NEXT_WORKING_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectoryEvidence {
    pub repository_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_selector: Option<String>,
    pub resolved_commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_tree: Option<String>,
    pub materializer_kind: MaterializerKind,
    pub dirty_state_policy: DirtyStatePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectory {
    pub id: String,
    pub repository_id: String,
    pub materializer_kind: MaterializerKind,
    pub dirty_state_policy: DirtyStatePolicy,
    pub evidence: WorkingDirectoryEvidence,
    pub cleanup_target: WorkingDirectoryCleanupTarget,
    pub cleanup_policy: String,
    pub status: WorkingDirectoryStatusKind,
}

impl WorkingDirectory {
    pub fn status_summary(&self) -> WorkingDirectorySummary {
        WorkingDirectorySummary {
            working_directory_id: self.id.clone(),
            repository_id: self.repository_id.clone(),
            requested_selector: self.evidence.requested_selector.clone(),
            materializer_kind: self.materializer_kind.clone(),
            dirty_state_policy: self.dirty_state_policy.clone(),
            resolved_commit: Some(self.evidence.resolved_commit.clone()),
            resolved_tree: self.evidence.resolved_tree.clone(),
            cleanup_target: Some(self.cleanup_target.clone()),
            cleanup_policy: Some(self.cleanup_policy.clone()),
            status: self.status.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingDirectoryBinding {
    pub working_directory: WorkingDirectory,
    pub root: PathBuf,
    pub cwd: PathBuf,
    working_directory_root: PathBuf,
    source_repository_path: PathBuf,
}

impl WorkingDirectoryBinding {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn working_directory_root(&self) -> &Path {
        &self.working_directory_root
    }

    pub fn source_repository_path(&self) -> &Path {
        &self.source_repository_path
    }

    pub fn status(&self) -> WorkingDirectoryStatus {
        WorkingDirectoryStatus {
            summary: self.working_directory.status_summary(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingDirectoryDiagnostic {
    pub code: String,
    pub message: String,
}

impl WorkingDirectoryDiagnostic {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WorkingDirectoryDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for WorkingDirectoryDiagnostic {}

pub trait WorkingDirectoryMaterializer: Send + Sync + 'static {
    fn materialize(
        &self,
        worker_ref: &WorkerRef,
        request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic>;

    fn create(
        &self,
        request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic>;

    fn bind_working_directory(
        &self,
        working_directory_id: &str,
        relative_cwd: Option<&str>,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic>;

    fn list_working_directories(
        &self,
    ) -> Result<Vec<WorkingDirectoryStatus>, WorkingDirectoryDiagnostic>;

    fn working_directory_status(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic>;

    fn cleanup_working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic>;

    fn cleanup(&self, binding: &WorkingDirectoryBinding) -> Result<(), WorkingDirectoryDiagnostic>;
}

#[derive(Clone, Debug)]
pub struct LocalGitWorktreeMaterializer {
    runtime_root: PathBuf,
}

impl LocalGitWorktreeMaterializer {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
        }
    }

    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    fn working_directory_id(worker_ref: &WorkerRef, repository_id: &str) -> String {
        format!(
            "{}-{}-{}",
            sanitize_path_component(worker_ref.runtime_id.as_str()),
            sanitize_path_component(worker_ref.worker_id.as_str()),
            sanitize_path_component(repository_id)
        )
    }

    fn working_directory_root(&self, working_directory_id: &str) -> PathBuf {
        self.runtime_root
            .join(WORKING_DIRECTORIES_DIR)
            .join(working_directory_id)
    }

    fn write_record(
        &self,
        binding: &WorkingDirectoryBinding,
    ) -> Result<(), WorkingDirectoryDiagnostic> {
        let record = WorkingDirectoryMaterializationRecord {
            working_directory: binding.working_directory.clone(),
            root: binding.root.clone(),
            source_repository_path: binding.source_repository_path.clone(),
        };
        let path = binding.working_directory_root.join(MATERIALIZATION_RECORD);
        let raw = serde_json::to_vec_pretty(&record).map_err(|error| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_record_serialize_failed",
                format!("failed to serialize working directory record: {error}"),
            )
        })?;
        fs::write(&path, raw).map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_record_write_failed",
                "failed to write working directory record; backend-private path details were omitted",
            )
        })
    }

    fn read_binding(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        let working_directory_root = self.working_directory_root(working_directory_id);
        let path = working_directory_root.join(MATERIALIZATION_RECORD);
        let raw = fs::read(&path).map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_not_found",
                "working directory working_directory was not found",
            )
        })?;
        let record: WorkingDirectoryMaterializationRecord = serde_json::from_slice(&raw).map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_record_invalid",
                "working directory working_directory record is invalid; backend-private path details were omitted",
            )
        })?;
        Ok(WorkingDirectoryBinding {
            working_directory: record.working_directory,
            root: record.root.clone(),
            cwd: record.root,
            working_directory_root,
            source_repository_path: record.source_repository_path,
        })
    }

    fn materialize_with_working_directory_id(
        &self,
        working_directory_id: String,
        request: &WorkingDirectoryRequest,
        cleanup_policy: &str,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        validate_working_directory_id(&working_directory_id)?;
        if request.materializer != MaterializerKind::LocalGitWorktree {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_materializer_unsupported",
                "only local_git_worktree working directory materialization is supported in v0",
            ));
        }
        if request.repository.provider != "git" {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_repository_provider_unsupported",
                format!(
                    "repository provider `{}` is not supported by the v0 working directory materializer",
                    request.repository.provider
                ),
            ));
        }
        if request.dirty_state_policy != DirtyStatePolicy::CleanPointOnly {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_dirty_policy_unsupported",
                "only clean_point_only dirty-state policy is supported in v0",
            ));
        }
        if is_remote_uri(&request.repository.uri) {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_remote_repository_unsupported",
                "remote repository URI materialization is not implemented in v0",
            ));
        }

        let source_path = request
            .repository
            .local_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(&request.repository.uri));
        let source_root = git_stdout(&source_path, ["rev-parse", "--show-toplevel"])
            .map(|value| PathBuf::from(value.trim()))
            .map_err(|_| {
                WorkingDirectoryDiagnostic::new(
                    "working_directory_git_repository_unavailable",
                    "configured local repository is not an available Git worktree; backend-private path details were omitted",
                )
            })?;

        let status = git_stdout(&source_root, ["status", "--porcelain"])?;
        if !status.trim().is_empty() {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_dirty_source_rejected",
                "clean_point_only working directory materialization rejects dirty source repository state",
            ));
        }

        let selector = request
            .repository
            .selector
            .as_deref()
            .unwrap_or("HEAD")
            .to_string();
        let commit_spec = format!("{selector}^{{commit}}");
        let resolved_commit = git_stdout(&source_root, ["rev-parse", commit_spec.as_str()])?
            .trim()
            .to_string();
        let tree_spec = format!("{resolved_commit}^{{tree}}");
        let resolved_tree = git_stdout(&source_root, ["rev-parse", tree_spec.as_str()])
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let working_directory_root = self.working_directory_root(&working_directory_id);
        let worktree_root = working_directory_root
            .join("root")
            .join(sanitize_path_component(&request.repository.id));
        if worktree_root.exists() {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_exists",
                "working directory working_directory target already exists; cleanup or choose a new working_directory",
            ));
        }
        fs::create_dir_all(worktree_root.parent().ok_or_else(|| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_invalid_target",
                "working directory working_directory target has no parent directory",
            )
        })?)
        .map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_create_failed",
                "failed to create working directory working_directory directory; backend-private path details were omitted",
            )
        })?;

        let workspace_worktree_root_arg = path_str(&worktree_root)?;
        git_status(
            &source_root,
            [
                "worktree",
                "add",
                "--detach",
                workspace_worktree_root_arg.as_str(),
                resolved_commit.as_str(),
            ],
        )?;

        let working_directory = WorkingDirectory {
            id: working_directory_id.clone(),
            repository_id: request.repository.id.clone(),
            materializer_kind: MaterializerKind::LocalGitWorktree,
            dirty_state_policy: DirtyStatePolicy::CleanPointOnly,
            evidence: WorkingDirectoryEvidence {
                repository_id: request.repository.id.clone(),
                requested_selector: request
                    .repository
                    .selector
                    .as_ref()
                    .map(|selector| selector.as_ref().to_string()),
                resolved_commit,
                resolved_tree,
                materializer_kind: MaterializerKind::LocalGitWorktree,
                dirty_state_policy: DirtyStatePolicy::CleanPointOnly,
            },
            cleanup_target: WorkingDirectoryCleanupTarget {
                kind: "git_worktree".to_string(),
                working_directory_id,
                repository_id: request.repository.id.clone(),
            },
            cleanup_policy: cleanup_policy.to_string(),
            status: WorkingDirectoryStatusKind::Active,
        };
        let binding = WorkingDirectoryBinding {
            working_directory,
            root: worktree_root.clone(),
            cwd: worktree_root,
            working_directory_root,
            source_repository_path: source_root,
        };
        self.write_record(&binding)?;
        Ok(binding)
    }
}

impl WorkingDirectoryMaterializer for LocalGitWorktreeMaterializer {
    fn materialize(
        &self,
        worker_ref: &WorkerRef,
        request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        let working_directory_id = Self::working_directory_id(worker_ref, &request.repository.id);
        self.materialize_with_working_directory_id(
            working_directory_id,
            request,
            "remove_on_worker_stop",
        )
    }

    fn create(
        &self,
        request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        let working_directory_id = next_working_directory_id(&request.repository.id);
        self.materialize_with_working_directory_id(
            working_directory_id,
            request,
            "manual_or_worker_stop",
        )
    }

    fn bind_working_directory(
        &self,
        working_directory_id: &str,
        relative_cwd: Option<&str>,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        validate_working_directory_id(working_directory_id)?;
        let binding = self.read_binding(working_directory_id)?;
        if binding.working_directory.status != WorkingDirectoryStatusKind::Active {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_not_active",
                "working directory working_directory is not active",
            ));
        }
        let cwd = validate_relative_cwd(binding.root(), relative_cwd)?;
        Ok(WorkingDirectoryBinding { cwd, ..binding })
    }

    fn list_working_directories(
        &self,
    ) -> Result<Vec<WorkingDirectoryStatus>, WorkingDirectoryDiagnostic> {
        let root = self.runtime_root.join(WORKING_DIRECTORIES_DIR);
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => {
                return Err(WorkingDirectoryDiagnostic::new(
                    "working_directory_list_failed",
                    "failed to list working directory working_directories; backend-private path details were omitted",
                ));
            }
        };
        let mut statuses = Vec::new();
        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if !file_type.is_dir() {
                continue;
            }
            let working_directory_id = entry.file_name().to_string_lossy().to_string();
            if validate_working_directory_id(&working_directory_id).is_err() {
                continue;
            }
            if let Ok(status) = self.working_directory_status(&working_directory_id) {
                statuses.push(status);
            }
        }
        statuses.sort_by(|left, right| {
            left.summary
                .working_directory_id
                .cmp(&right.summary.working_directory_id)
        });
        Ok(statuses)
    }

    fn working_directory_status(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        validate_working_directory_id(working_directory_id)?;
        Ok(self.read_binding(working_directory_id)?.status())
    }

    fn cleanup_working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        validate_working_directory_id(working_directory_id)?;
        let binding = self.read_binding(working_directory_id)?;
        self.cleanup(&binding)?;
        self.working_directory_status(working_directory_id)
    }

    fn cleanup(&self, binding: &WorkingDirectoryBinding) -> Result<(), WorkingDirectoryDiagnostic> {
        let mut working_directory = binding.working_directory.clone();
        let working_directory_root = binding.working_directory_root.canonicalize().map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_cleanup_target_invalid",
                "working directory working directory root is unavailable; backend-private path details were omitted",
            )
        })?;
        let root = binding.root.canonicalize().map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_cleanup_target_invalid",
                "working directory root is unavailable; backend-private path details were omitted",
            )
        })?;
        if !root.starts_with(&working_directory_root) {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_cleanup_escape_rejected",
                "working directory cleanup target is outside the working directory root",
            ));
        }
        let workspace_worktree_root_arg = path_str(&root)?;
        let remove_result = git_status(
            binding.source_repository_path(),
            ["worktree", "remove", "--force", workspace_worktree_root_arg.as_str()],
        )
        .or_else(|_| {
            if root.exists() {
                fs::remove_dir_all(&root).map_err(|_| {
                    WorkingDirectoryDiagnostic::new(
                        "working_directory_cleanup_failed",
                        "failed to remove working directory; backend-private path details were omitted",
                    )
                })
            } else {
                Ok(())
            }
        });
        working_directory.status = if remove_result.is_ok() {
            WorkingDirectoryStatusKind::Removed
        } else {
            WorkingDirectoryStatusKind::CleanupPending
        };
        let updated = WorkingDirectoryBinding {
            working_directory,
            root: binding.root.clone(),
            cwd: binding.cwd.clone(),
            working_directory_root: binding.working_directory_root.clone(),
            source_repository_path: binding.source_repository_path.clone(),
        };
        let _ = self.write_record(&updated);
        remove_result
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct WorkingDirectoryMaterializationRecord {
    working_directory: WorkingDirectory,
    root: PathBuf,
    source_repository_path: PathBuf,
}

fn git_stdout<'a, I>(repository_path: &Path, args: I) -> Result<String, WorkingDirectoryDiagnostic>
where
    I: IntoIterator<Item = &'a str>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_path)
        .args(args)
        .output()
        .map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_git_unavailable",
                "Git command could not be executed; backend-private path details were omitted",
            )
        })?;
    if !output.status.success() {
        return Err(WorkingDirectoryDiagnostic::new(
            "working_directory_git_failed",
            "Git command failed; backend-private path details were omitted",
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_status<'a, I>(repository_path: &Path, args: I) -> Result<(), WorkingDirectoryDiagnostic>
where
    I: IntoIterator<Item = &'a str>,
{
    git_stdout(repository_path, args).map(|_| ())
}

fn path_str(path: &Path) -> Result<String, WorkingDirectoryDiagnostic> {
    path.to_str().map(ToString::to_string).ok_or_else(|| {
        WorkingDirectoryDiagnostic::new(
            "working_directory_non_utf8_path",
            "working directory path is not valid UTF-8; backend-private path details were omitted",
        )
    })
}

fn is_remote_uri(uri: &str) -> bool {
    uri.contains("://") || uri.starts_with("git@") || uri.starts_with("ssh:")
}

fn sanitize_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches(['-', '.']).to_string();
    if trimmed.is_empty() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        format!("workspace-{now}")
    } else {
        trimmed
    }
}

fn next_working_directory_id(repository_id: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let sequence = NEXT_WORKING_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "alloc-{now}-{sequence}-{}",
        sanitize_path_component(repository_id)
    )
}

fn validate_working_directory_id(
    working_directory_id: &str,
) -> Result<(), WorkingDirectoryDiagnostic> {
    let sanitized = sanitize_path_component(working_directory_id);
    if working_directory_id.is_empty()
        || working_directory_id != sanitized
        || working_directory_id.contains(std::path::MAIN_SEPARATOR)
        || working_directory_id.contains('/')
        || working_directory_id.contains('\\')
    {
        return Err(WorkingDirectoryDiagnostic::new(
            "working_directory_id_invalid",
            "working directory working_directory id is invalid",
        ));
    }
    Ok(())
}

fn validate_relative_cwd(
    root: &Path,
    relative_cwd: Option<&str>,
) -> Result<PathBuf, WorkingDirectoryDiagnostic> {
    let root = root.canonicalize().map_err(|_| {
        WorkingDirectoryDiagnostic::new(
            "working_directory_root_unavailable",
            "working directory root is unavailable; backend-private path details were omitted",
        )
    })?;
    let relative = relative_cwd.unwrap_or(".").trim();
    if relative.is_empty() || relative == "." {
        return Ok(root);
    }
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(WorkingDirectoryDiagnostic::new(
            "working_directory_relative_cwd_invalid",
            "working directory relative_cwd must be a relative path inside the working directory root",
        ));
    }
    let target = root.join(relative_path).canonicalize().map_err(|_| {
        WorkingDirectoryDiagnostic::new(
            "working_directory_relative_cwd_unavailable",
            "working directory relative_cwd does not identify an existing directory",
        )
    })?;
    if !target.starts_with(&root) || !target.is_dir() {
        return Err(WorkingDirectoryDiagnostic::new(
            "working_directory_relative_cwd_escape_rejected",
            "working directory relative_cwd must resolve to a directory inside the working directory root",
        ));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{RepositorySelector, WorkingDirectoryRepository};
    use crate::identity::{RuntimeId, WorkerId};

    fn git(path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    fn create_clean_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]);
        git(
            dir.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(dir.path(), &["config", "user.name", "Yoi Test"]);
        fs::write(dir.path().join("README.md"), "clean\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        git(dir.path(), &["commit", "-m", "init"]);
        dir
    }

    fn request(repo: &Path) -> WorkingDirectoryRequest {
        WorkingDirectoryRequest {
            repository: WorkingDirectoryRepository {
                id: "repo-main".to_string(),
                provider: "git".to_string(),
                uri: ".".to_string(),
                local_path: Some(repo.to_path_buf()),
                selector: Some(RepositorySelector::from("HEAD")),
            },
            materializer: MaterializerKind::LocalGitWorktree,
            dirty_state_policy: DirtyStatePolicy::CleanPointOnly,
        }
    }

    fn worker_ref(sequence: u64) -> WorkerRef {
        WorkerRef::new(
            RuntimeId::new("runtime-test").unwrap(),
            WorkerId::generated(sequence),
        )
    }

    #[test]
    fn local_git_repo_materializes_detached_worktree_under_runtime_root() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = LocalGitWorktreeMaterializer::new(runtime_root.path());
        let binding = materializer
            .materialize(&worker_ref(1), &request(repo.path()))
            .unwrap();

        assert!(binding.root.starts_with(runtime_root.path()));
        assert!(binding.root.join("README.md").exists());
        let branch = git_stdout(binding.root(), ["branch", "--show-current"]).unwrap();
        assert!(
            branch.is_empty(),
            "worktree should be detached, got {branch}"
        );
        assert_eq!(
            binding.working_directory.materializer_kind,
            MaterializerKind::LocalGitWorktree
        );
        assert_eq!(
            binding.working_directory.dirty_state_policy,
            DirtyStatePolicy::CleanPointOnly
        );
        assert!(
            binding
                .working_directory_root()
                .join(MATERIALIZATION_RECORD)
                .exists()
        );
    }

    #[test]
    fn multiple_workers_materialize_distinct_paths_for_same_source_repo() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = LocalGitWorktreeMaterializer::new(runtime_root.path());
        let first = materializer
            .materialize(&worker_ref(1), &request(repo.path()))
            .unwrap();
        let second = materializer
            .materialize(&worker_ref(2), &request(repo.path()))
            .unwrap();

        assert_ne!(first.root, second.root);
        assert!(first.root.starts_with(runtime_root.path()));
        assert!(second.root.starts_with(runtime_root.path()));
        assert!(!first.root.starts_with(repo.path()));
        assert!(!second.root.starts_with(repo.path()));
    }

    #[test]
    fn dirty_source_is_rejected_by_clean_point_only_policy() {
        let repo = create_clean_repo();
        fs::write(repo.path().join("dirty.txt"), "dirty\n").unwrap();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = LocalGitWorktreeMaterializer::new(runtime_root.path());

        let error = materializer
            .materialize(&worker_ref(1), &request(repo.path()))
            .unwrap_err();

        assert_eq!(error.code, "working_directory_dirty_source_rejected");
        assert!(error.message.contains("clean_point_only"));
    }

    #[test]
    fn unsupported_remote_and_non_git_provider_return_typed_diagnostics() {
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = LocalGitWorktreeMaterializer::new(runtime_root.path());
        let mut remote = request(Path::new("."));
        remote.repository.local_path = None;
        remote.repository.uri = "https://example.invalid/repo.git".to_string();
        let error = materializer
            .materialize(&worker_ref(1), &remote)
            .unwrap_err();
        assert_eq!(
            error.code,
            "working_directory_remote_repository_unsupported"
        );

        let mut non_git = remote;
        non_git.repository.provider = "archive".to_string();
        non_git.repository.uri = ".".to_string();
        let error = materializer
            .materialize(&worker_ref(2), &non_git)
            .unwrap_err();
        assert_eq!(
            error.code,
            "working_directory_repository_provider_unsupported"
        );
    }

    #[test]
    fn working_directory_binds_safe_relative_cwd_and_lists_without_paths() {
        let repo = create_clean_repo();
        fs::create_dir_all(repo.path().join("crates/yoi")).unwrap();
        fs::write(repo.path().join("crates/yoi/lib.rs"), "// ok\n").unwrap();
        git(repo.path(), &["add", "crates/yoi/lib.rs"]);
        git(repo.path(), &["commit", "-m", "add crate"]);
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = LocalGitWorktreeMaterializer::new(runtime_root.path());
        let working_directory = materializer.create(&request(repo.path())).unwrap();

        let bound = materializer
            .bind_working_directory(&working_directory.working_directory.id, Some("crates/yoi"))
            .unwrap();
        assert_eq!(bound.cwd.file_name().unwrap(), "yoi");
        let listed = materializer.list_working_directories().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].summary.working_directory_id,
            working_directory.working_directory.id
        );
        assert_eq!(
            listed[0].summary.requested_selector.as_deref(),
            Some("HEAD")
        );
    }

    #[test]
    fn relative_cwd_rejects_absolute_parent_nonexistent_file_and_symlink_escape() {
        let repo = create_clean_repo();
        fs::create_dir_all(repo.path().join("inside")).unwrap();
        fs::write(repo.path().join("inside/file.txt"), "file\n").unwrap();
        git(repo.path(), &["add", "inside/file.txt"]);
        git(repo.path(), &["commit", "-m", "add inside"]);
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = LocalGitWorktreeMaterializer::new(runtime_root.path());
        let working_directory = materializer.create(&request(repo.path())).unwrap();

        assert_eq!(
            materializer
                .bind_working_directory(&working_directory.working_directory.id, Some("/tmp"))
                .unwrap_err()
                .code,
            "working_directory_relative_cwd_invalid"
        );
        assert_eq!(
            materializer
                .bind_working_directory(&working_directory.working_directory.id, Some("../outside"))
                .unwrap_err()
                .code,
            "working_directory_relative_cwd_invalid"
        );
        assert_eq!(
            materializer
                .bind_working_directory(&working_directory.working_directory.id, Some("missing"))
                .unwrap_err()
                .code,
            "working_directory_relative_cwd_unavailable"
        );
        assert_eq!(
            materializer
                .bind_working_directory(
                    &working_directory.working_directory.id,
                    Some("inside/file.txt")
                )
                .unwrap_err()
                .code,
            "working_directory_relative_cwd_escape_rejected"
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/tmp", working_directory.root().join("escape")).unwrap();
            assert_eq!(
                materializer
                    .bind_working_directory(&working_directory.working_directory.id, Some("escape"))
                    .unwrap_err()
                    .code,
                "working_directory_relative_cwd_escape_rejected"
            );
        }
    }

    #[test]
    fn cleanup_removes_worktree_and_updates_record() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = LocalGitWorktreeMaterializer::new(runtime_root.path());
        let binding = materializer
            .materialize(&worker_ref(1), &request(repo.path()))
            .unwrap();
        let root = binding.root.clone();

        materializer.cleanup(&binding).unwrap();

        assert!(!root.exists());
        let raw = fs::read_to_string(
            binding
                .working_directory_root()
                .join(MATERIALIZATION_RECORD),
        )
        .unwrap();
        assert!(raw.contains("removed"));
    }
}
