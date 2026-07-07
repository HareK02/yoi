use crate::catalog::{
    DirtyStatePolicy, ExecutionWorkspaceCleanupTarget, ExecutionWorkspaceRequest,
    ExecutionWorkspaceStatus, ExecutionWorkspaceStatusKind, ExecutionWorkspaceSummary,
    MaterializerKind,
};
use crate::identity::WorkerRef;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const EXECUTION_WORKSPACES_DIR: &str = "execution-workspaces";
const MATERIALIZATION_RECORD: &str = "materialization.json";
static NEXT_ALLOCATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionWorkspaceEvidence {
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
pub struct ExecutionWorkspaceAllocation {
    pub id: String,
    pub repository_id: String,
    pub materializer_kind: MaterializerKind,
    pub dirty_state_policy: DirtyStatePolicy,
    pub evidence: ExecutionWorkspaceEvidence,
    pub cleanup_target: ExecutionWorkspaceCleanupTarget,
    pub cleanup_policy: String,
    pub status: ExecutionWorkspaceStatusKind,
}

impl ExecutionWorkspaceAllocation {
    pub fn status_summary(&self) -> ExecutionWorkspaceSummary {
        ExecutionWorkspaceSummary {
            allocation_id: self.id.clone(),
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
pub struct ExecutionWorkspaceBinding {
    pub allocation: ExecutionWorkspaceAllocation,
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    allocation_root: PathBuf,
    source_repository_path: PathBuf,
}

impl ExecutionWorkspaceBinding {
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn allocation_root(&self) -> &Path {
        &self.allocation_root
    }

    pub fn source_repository_path(&self) -> &Path {
        &self.source_repository_path
    }

    pub fn status(&self) -> ExecutionWorkspaceStatus {
        ExecutionWorkspaceStatus {
            summary: self.allocation.status_summary(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionWorkspaceDiagnostic {
    pub code: String,
    pub message: String,
}

impl ExecutionWorkspaceDiagnostic {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ExecutionWorkspaceDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ExecutionWorkspaceDiagnostic {}

pub trait ExecutionWorkspaceMaterializer: Send + Sync + 'static {
    fn materialize(
        &self,
        worker_ref: &WorkerRef,
        request: &ExecutionWorkspaceRequest,
    ) -> Result<ExecutionWorkspaceBinding, ExecutionWorkspaceDiagnostic>;

    fn preallocate(
        &self,
        request: &ExecutionWorkspaceRequest,
    ) -> Result<ExecutionWorkspaceBinding, ExecutionWorkspaceDiagnostic>;

    fn bind_allocation(
        &self,
        allocation_id: &str,
        relative_cwd: Option<&str>,
    ) -> Result<ExecutionWorkspaceBinding, ExecutionWorkspaceDiagnostic>;

    fn list_allocations(
        &self,
    ) -> Result<Vec<ExecutionWorkspaceStatus>, ExecutionWorkspaceDiagnostic>;

    fn allocation_status(
        &self,
        allocation_id: &str,
    ) -> Result<ExecutionWorkspaceStatus, ExecutionWorkspaceDiagnostic>;

    fn cleanup_allocation(
        &self,
        allocation_id: &str,
    ) -> Result<ExecutionWorkspaceStatus, ExecutionWorkspaceDiagnostic>;

    fn cleanup(
        &self,
        binding: &ExecutionWorkspaceBinding,
    ) -> Result<(), ExecutionWorkspaceDiagnostic>;
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

    fn allocation_id(worker_ref: &WorkerRef, repository_id: &str) -> String {
        format!(
            "{}-{}-{}",
            sanitize_path_component(worker_ref.runtime_id.as_str()),
            sanitize_path_component(worker_ref.worker_id.as_str()),
            sanitize_path_component(repository_id)
        )
    }

    fn allocation_root(&self, allocation_id: &str) -> PathBuf {
        self.runtime_root
            .join(EXECUTION_WORKSPACES_DIR)
            .join(allocation_id)
    }

    fn write_record(
        &self,
        binding: &ExecutionWorkspaceBinding,
    ) -> Result<(), ExecutionWorkspaceDiagnostic> {
        let record = ExecutionWorkspaceMaterializationRecord {
            allocation: binding.allocation.clone(),
            workspace_root: binding.workspace_root.clone(),
            source_repository_path: binding.source_repository_path.clone(),
        };
        let path = binding.allocation_root.join(MATERIALIZATION_RECORD);
        let raw = serde_json::to_vec_pretty(&record).map_err(|error| {
            ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_record_serialize_failed",
                format!("failed to serialize execution workspace record: {error}"),
            )
        })?;
        fs::write(&path, raw).map_err(|_| {
            ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_record_write_failed",
                "failed to write execution workspace record; backend-private path details were omitted",
            )
        })
    }

    fn read_binding(
        &self,
        allocation_id: &str,
    ) -> Result<ExecutionWorkspaceBinding, ExecutionWorkspaceDiagnostic> {
        let allocation_root = self.allocation_root(allocation_id);
        let path = allocation_root.join(MATERIALIZATION_RECORD);
        let raw = fs::read(&path).map_err(|_| {
            ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_allocation_not_found",
                "execution workspace allocation was not found",
            )
        })?;
        let record: ExecutionWorkspaceMaterializationRecord = serde_json::from_slice(&raw).map_err(|_| {
            ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_record_invalid",
                "execution workspace allocation record is invalid; backend-private path details were omitted",
            )
        })?;
        Ok(ExecutionWorkspaceBinding {
            allocation: record.allocation,
            workspace_root: record.workspace_root.clone(),
            cwd: record.workspace_root,
            allocation_root,
            source_repository_path: record.source_repository_path,
        })
    }

    fn materialize_with_allocation_id(
        &self,
        allocation_id: String,
        request: &ExecutionWorkspaceRequest,
        cleanup_policy: &str,
    ) -> Result<ExecutionWorkspaceBinding, ExecutionWorkspaceDiagnostic> {
        validate_allocation_id(&allocation_id)?;
        if request.materializer != MaterializerKind::LocalGitWorktree {
            return Err(ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_materializer_unsupported",
                "only local_git_worktree execution workspace materialization is supported in v0",
            ));
        }
        if request.repository.provider != "git" {
            return Err(ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_repository_provider_unsupported",
                format!(
                    "repository provider `{}` is not supported by the v0 execution workspace materializer",
                    request.repository.provider
                ),
            ));
        }
        if request.dirty_state_policy != DirtyStatePolicy::CleanPointOnly {
            return Err(ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_dirty_policy_unsupported",
                "only clean_point_only dirty-state policy is supported in v0",
            ));
        }
        if is_remote_uri(&request.repository.uri) {
            return Err(ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_remote_repository_unsupported",
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
                ExecutionWorkspaceDiagnostic::new(
                    "execution_workspace_git_repository_unavailable",
                    "configured local repository is not an available Git worktree; backend-private path details were omitted",
                )
            })?;

        let status = git_stdout(&source_root, ["status", "--porcelain"])?;
        if !status.trim().is_empty() {
            return Err(ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_dirty_source_rejected",
                "clean_point_only execution workspace materialization rejects dirty source repository state",
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

        let allocation_root = self.allocation_root(&allocation_id);
        let workspace_root = allocation_root
            .join("root")
            .join(sanitize_path_component(&request.repository.id));
        if workspace_root.exists() {
            return Err(ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_allocation_exists",
                "execution workspace allocation target already exists; cleanup or choose a new allocation",
            ));
        }
        fs::create_dir_all(workspace_root.parent().ok_or_else(|| {
            ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_invalid_target",
                "execution workspace allocation target has no parent directory",
            )
        })?)
        .map_err(|_| {
            ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_create_failed",
                "failed to create execution workspace allocation directory; backend-private path details were omitted",
            )
        })?;

        let workspace_root_arg = path_str(&workspace_root)?;
        git_status(
            &source_root,
            [
                "worktree",
                "add",
                "--detach",
                workspace_root_arg.as_str(),
                resolved_commit.as_str(),
            ],
        )?;

        let allocation = ExecutionWorkspaceAllocation {
            id: allocation_id.clone(),
            repository_id: request.repository.id.clone(),
            materializer_kind: MaterializerKind::LocalGitWorktree,
            dirty_state_policy: DirtyStatePolicy::CleanPointOnly,
            evidence: ExecutionWorkspaceEvidence {
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
            cleanup_target: ExecutionWorkspaceCleanupTarget {
                kind: "git_worktree".to_string(),
                allocation_id,
                repository_id: request.repository.id.clone(),
            },
            cleanup_policy: cleanup_policy.to_string(),
            status: ExecutionWorkspaceStatusKind::Active,
        };
        let binding = ExecutionWorkspaceBinding {
            allocation,
            workspace_root: workspace_root.clone(),
            cwd: workspace_root,
            allocation_root,
            source_repository_path: source_root,
        };
        self.write_record(&binding)?;
        Ok(binding)
    }
}

impl ExecutionWorkspaceMaterializer for LocalGitWorktreeMaterializer {
    fn materialize(
        &self,
        worker_ref: &WorkerRef,
        request: &ExecutionWorkspaceRequest,
    ) -> Result<ExecutionWorkspaceBinding, ExecutionWorkspaceDiagnostic> {
        let allocation_id = Self::allocation_id(worker_ref, &request.repository.id);
        self.materialize_with_allocation_id(allocation_id, request, "remove_on_worker_stop")
    }

    fn preallocate(
        &self,
        request: &ExecutionWorkspaceRequest,
    ) -> Result<ExecutionWorkspaceBinding, ExecutionWorkspaceDiagnostic> {
        let allocation_id = next_allocation_id(&request.repository.id);
        self.materialize_with_allocation_id(allocation_id, request, "manual_or_worker_stop")
    }

    fn bind_allocation(
        &self,
        allocation_id: &str,
        relative_cwd: Option<&str>,
    ) -> Result<ExecutionWorkspaceBinding, ExecutionWorkspaceDiagnostic> {
        validate_allocation_id(allocation_id)?;
        let binding = self.read_binding(allocation_id)?;
        if binding.allocation.status != ExecutionWorkspaceStatusKind::Active {
            return Err(ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_not_active",
                "execution workspace allocation is not active",
            ));
        }
        let cwd = validate_relative_cwd(binding.workspace_root(), relative_cwd)?;
        Ok(ExecutionWorkspaceBinding { cwd, ..binding })
    }

    fn list_allocations(
        &self,
    ) -> Result<Vec<ExecutionWorkspaceStatus>, ExecutionWorkspaceDiagnostic> {
        let root = self.runtime_root.join(EXECUTION_WORKSPACES_DIR);
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => {
                return Err(ExecutionWorkspaceDiagnostic::new(
                    "execution_workspace_list_failed",
                    "failed to list execution workspace allocations; backend-private path details were omitted",
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
            let allocation_id = entry.file_name().to_string_lossy().to_string();
            if validate_allocation_id(&allocation_id).is_err() {
                continue;
            }
            if let Ok(status) = self.allocation_status(&allocation_id) {
                statuses.push(status);
            }
        }
        statuses
            .sort_by(|left, right| left.summary.allocation_id.cmp(&right.summary.allocation_id));
        Ok(statuses)
    }

    fn allocation_status(
        &self,
        allocation_id: &str,
    ) -> Result<ExecutionWorkspaceStatus, ExecutionWorkspaceDiagnostic> {
        validate_allocation_id(allocation_id)?;
        Ok(self.read_binding(allocation_id)?.status())
    }

    fn cleanup_allocation(
        &self,
        allocation_id: &str,
    ) -> Result<ExecutionWorkspaceStatus, ExecutionWorkspaceDiagnostic> {
        validate_allocation_id(allocation_id)?;
        let binding = self.read_binding(allocation_id)?;
        self.cleanup(&binding)?;
        self.allocation_status(allocation_id)
    }

    fn cleanup(
        &self,
        binding: &ExecutionWorkspaceBinding,
    ) -> Result<(), ExecutionWorkspaceDiagnostic> {
        let mut allocation = binding.allocation.clone();
        let allocation_root = binding.allocation_root.canonicalize().map_err(|_| {
            ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_cleanup_target_invalid",
                "execution workspace allocation root is unavailable; backend-private path details were omitted",
            )
        })?;
        let workspace_root = binding.workspace_root.canonicalize().map_err(|_| {
            ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_cleanup_target_invalid",
                "execution workspace root is unavailable; backend-private path details were omitted",
            )
        })?;
        if !workspace_root.starts_with(&allocation_root) {
            return Err(ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_cleanup_escape_rejected",
                "execution workspace cleanup target is outside the allocation root",
            ));
        }
        let workspace_root_arg = path_str(&workspace_root)?;
        let remove_result = git_status(
            binding.source_repository_path(),
            ["worktree", "remove", "--force", workspace_root_arg.as_str()],
        )
        .or_else(|_| {
            if workspace_root.exists() {
                fs::remove_dir_all(&workspace_root).map_err(|_| {
                    ExecutionWorkspaceDiagnostic::new(
                        "execution_workspace_cleanup_failed",
                        "failed to remove execution workspace; backend-private path details were omitted",
                    )
                })
            } else {
                Ok(())
            }
        });
        allocation.status = if remove_result.is_ok() {
            ExecutionWorkspaceStatusKind::Removed
        } else {
            ExecutionWorkspaceStatusKind::CleanupPending
        };
        let updated = ExecutionWorkspaceBinding {
            allocation,
            workspace_root: binding.workspace_root.clone(),
            cwd: binding.cwd.clone(),
            allocation_root: binding.allocation_root.clone(),
            source_repository_path: binding.source_repository_path.clone(),
        };
        let _ = self.write_record(&updated);
        remove_result
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ExecutionWorkspaceMaterializationRecord {
    allocation: ExecutionWorkspaceAllocation,
    workspace_root: PathBuf,
    source_repository_path: PathBuf,
}

fn git_stdout<'a, I>(
    repository_path: &Path,
    args: I,
) -> Result<String, ExecutionWorkspaceDiagnostic>
where
    I: IntoIterator<Item = &'a str>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_path)
        .args(args)
        .output()
        .map_err(|_| {
            ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_git_unavailable",
                "Git command could not be executed; backend-private path details were omitted",
            )
        })?;
    if !output.status.success() {
        return Err(ExecutionWorkspaceDiagnostic::new(
            "execution_workspace_git_failed",
            "Git command failed; backend-private path details were omitted",
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_status<'a, I>(repository_path: &Path, args: I) -> Result<(), ExecutionWorkspaceDiagnostic>
where
    I: IntoIterator<Item = &'a str>,
{
    git_stdout(repository_path, args).map(|_| ())
}

fn path_str(path: &Path) -> Result<String, ExecutionWorkspaceDiagnostic> {
    path.to_str().map(ToString::to_string).ok_or_else(|| {
        ExecutionWorkspaceDiagnostic::new(
            "execution_workspace_non_utf8_path",
            "execution workspace path is not valid UTF-8; backend-private path details were omitted",
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

fn next_allocation_id(repository_id: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let sequence = NEXT_ALLOCATION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "alloc-{now}-{sequence}-{}",
        sanitize_path_component(repository_id)
    )
}

fn validate_allocation_id(allocation_id: &str) -> Result<(), ExecutionWorkspaceDiagnostic> {
    let sanitized = sanitize_path_component(allocation_id);
    if allocation_id.is_empty()
        || allocation_id != sanitized
        || allocation_id.contains(std::path::MAIN_SEPARATOR)
        || allocation_id.contains('/')
        || allocation_id.contains('\\')
    {
        return Err(ExecutionWorkspaceDiagnostic::new(
            "execution_workspace_allocation_id_invalid",
            "execution workspace allocation id is invalid",
        ));
    }
    Ok(())
}

fn validate_relative_cwd(
    workspace_root: &Path,
    relative_cwd: Option<&str>,
) -> Result<PathBuf, ExecutionWorkspaceDiagnostic> {
    let workspace_root = workspace_root.canonicalize().map_err(|_| {
        ExecutionWorkspaceDiagnostic::new(
            "execution_workspace_root_unavailable",
            "execution workspace root is unavailable; backend-private path details were omitted",
        )
    })?;
    let relative = relative_cwd.unwrap_or(".").trim();
    if relative.is_empty() || relative == "." {
        return Ok(workspace_root);
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
        return Err(ExecutionWorkspaceDiagnostic::new(
            "execution_workspace_relative_cwd_invalid",
            "execution workspace relative_cwd must be a relative path inside the allocation root",
        ));
    }
    let target = workspace_root
        .join(relative_path)
        .canonicalize()
        .map_err(|_| {
            ExecutionWorkspaceDiagnostic::new(
                "execution_workspace_relative_cwd_unavailable",
                "execution workspace relative_cwd does not identify an existing directory",
            )
        })?;
    if !target.starts_with(&workspace_root) || !target.is_dir() {
        return Err(ExecutionWorkspaceDiagnostic::new(
            "execution_workspace_relative_cwd_escape_rejected",
            "execution workspace relative_cwd must resolve to a directory inside the allocation root",
        ));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{ExecutionWorkspaceRepository, RepositorySelector};
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

    fn request(repo: &Path) -> ExecutionWorkspaceRequest {
        ExecutionWorkspaceRequest {
            repository: ExecutionWorkspaceRepository {
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

        assert!(binding.workspace_root.starts_with(runtime_root.path()));
        assert!(binding.workspace_root.join("README.md").exists());
        let branch = git_stdout(binding.workspace_root(), ["branch", "--show-current"]).unwrap();
        assert!(
            branch.is_empty(),
            "worktree should be detached, got {branch}"
        );
        assert_eq!(
            binding.allocation.materializer_kind,
            MaterializerKind::LocalGitWorktree
        );
        assert_eq!(
            binding.allocation.dirty_state_policy,
            DirtyStatePolicy::CleanPointOnly
        );
        assert!(
            binding
                .allocation_root()
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

        assert_ne!(first.workspace_root, second.workspace_root);
        assert!(first.workspace_root.starts_with(runtime_root.path()));
        assert!(second.workspace_root.starts_with(runtime_root.path()));
        assert!(!first.workspace_root.starts_with(repo.path()));
        assert!(!second.workspace_root.starts_with(repo.path()));
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

        assert_eq!(error.code, "execution_workspace_dirty_source_rejected");
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
            "execution_workspace_remote_repository_unsupported"
        );

        let mut non_git = remote;
        non_git.repository.provider = "archive".to_string();
        non_git.repository.uri = ".".to_string();
        let error = materializer
            .materialize(&worker_ref(2), &non_git)
            .unwrap_err();
        assert_eq!(
            error.code,
            "execution_workspace_repository_provider_unsupported"
        );
    }

    #[test]
    fn preallocated_allocation_binds_safe_relative_cwd_and_lists_without_paths() {
        let repo = create_clean_repo();
        fs::create_dir_all(repo.path().join("crates/yoi")).unwrap();
        fs::write(repo.path().join("crates/yoi/lib.rs"), "// ok\n").unwrap();
        git(repo.path(), &["add", "crates/yoi/lib.rs"]);
        git(repo.path(), &["commit", "-m", "add crate"]);
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = LocalGitWorktreeMaterializer::new(runtime_root.path());
        let allocation = materializer.preallocate(&request(repo.path())).unwrap();

        let bound = materializer
            .bind_allocation(&allocation.allocation.id, Some("crates/yoi"))
            .unwrap();
        assert_eq!(bound.cwd.file_name().unwrap(), "yoi");
        let listed = materializer.list_allocations().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].summary.allocation_id, allocation.allocation.id);
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
        let allocation = materializer.preallocate(&request(repo.path())).unwrap();

        assert_eq!(
            materializer
                .bind_allocation(&allocation.allocation.id, Some("/tmp"))
                .unwrap_err()
                .code,
            "execution_workspace_relative_cwd_invalid"
        );
        assert_eq!(
            materializer
                .bind_allocation(&allocation.allocation.id, Some("../outside"))
                .unwrap_err()
                .code,
            "execution_workspace_relative_cwd_invalid"
        );
        assert_eq!(
            materializer
                .bind_allocation(&allocation.allocation.id, Some("missing"))
                .unwrap_err()
                .code,
            "execution_workspace_relative_cwd_unavailable"
        );
        assert_eq!(
            materializer
                .bind_allocation(&allocation.allocation.id, Some("inside/file.txt"))
                .unwrap_err()
                .code,
            "execution_workspace_relative_cwd_escape_rejected"
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/tmp", allocation.workspace_root().join("escape")).unwrap();
            assert_eq!(
                materializer
                    .bind_allocation(&allocation.allocation.id, Some("escape"))
                    .unwrap_err()
                    .code,
                "execution_workspace_relative_cwd_escape_rejected"
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
        let workspace_root = binding.workspace_root.clone();

        materializer.cleanup(&binding).unwrap();

        assert!(!workspace_root.exists());
        let raw =
            fs::read_to_string(binding.allocation_root().join(MATERIALIZATION_RECORD)).unwrap();
        assert!(raw.contains("removed"));
    }
}
