use crate::catalog::{
    MaterializerKind, RepositorySshMaterializationAccess, WorkingDirectoryCleanupTarget,
    WorkingDirectoryRepositoryAccessRequest, WorkingDirectoryRequest, WorkingDirectoryStatus,
    WorkingDirectoryStatusKind, WorkingDirectorySummary,
};
use crate::identity::WorkerRef;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use workdir::WorkdirSessionResource;

const CHECKOUT_DIR: &str = "checkout";
const MATERIALIZATION_RECORD: &str = "materialization.json";
const REPOSITORY_CACHE_DIR: &str = ".repository-cache";
const REPOSITORY_ACCESS_DIR: &str = ".repository-access";
const REPOSITORY_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);
const REPOSITORY_MAX_OBJECTS: u64 = 5_000_000;
const REPOSITORY_MAX_BYTES: u64 = 5 * 1024 * 1024 * 1024;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_source_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_source_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_cache_key: Option<String>,
    #[serde(default)]
    pub cache_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_trust_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingDirectory {
    pub id: String,
    pub repository_id: String,
    pub materializer_kind: MaterializerKind,
    pub evidence: WorkingDirectoryEvidence,
    pub cleanup_target: WorkingDirectoryCleanupTarget,
    pub status: WorkingDirectoryStatusKind,
}

impl WorkingDirectory {
    pub fn status_summary(&self) -> WorkingDirectorySummary {
        WorkingDirectorySummary {
            working_directory_id: self.id.clone(),
            repository_id: self.repository_id.clone(),
            creation_selector: self.evidence.requested_selector.clone(),
            creation_ref: Some(self.evidence.resolved_commit.clone()),
            current_selector: None,
            current_ref: None,
            materializer_kind: self.materializer_kind.clone(),
            cleanup_target: Some(self.cleanup_target.clone()),
            status: self.status.clone(),
            cleanliness: None,
            primary_worker_id: None,
            occupied_by: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WorkingDirectoryBinding {
    pub working_directory: WorkingDirectory,
    pub root: PathBuf,
    pub cwd: PathBuf,
    working_directory_root: PathBuf,
    source_repository_path: PathBuf,
    command_environment: BTreeMap<String, String>,
    session_resources: Vec<Arc<dyn WorkdirSessionResource>>,
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

    pub fn command_environment(&self) -> BTreeMap<String, String> {
        self.command_environment.clone()
    }

    pub fn session_resources(&self) -> Vec<Arc<dyn WorkdirSessionResource>> {
        self.session_resources.clone()
    }

    pub fn status(&self) -> WorkingDirectoryStatus {
        let mut working_directory = self.working_directory.clone();
        if working_directory.status == WorkingDirectoryStatusKind::Active
            && !binding_paths_are_available(self)
        {
            working_directory.status = WorkingDirectoryStatusKind::Corrupted;
        }
        let mut summary = working_directory.status_summary();
        summary.cleanliness = if summary.status == WorkingDirectoryStatusKind::Active {
            let (current_selector, current_ref) = binding_current_revision(self);
            summary.current_selector = current_selector;
            summary.current_ref = current_ref;
            Some(binding_cleanliness(self))
        } else {
            Some("unknown".to_string())
        };
        WorkingDirectoryStatus { summary }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingDirectoryDiagnostic {
    pub code: String,
    pub message: String,
}

impl WorkingDirectoryDiagnostic {
    pub fn rejected(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message)
    }

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

    fn authorize_repository_access(
        &self,
        request: &WorkingDirectoryRepositoryAccessRequest,
    ) -> Result<(), WorkingDirectoryDiagnostic>;

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

fn binding_paths_are_available(binding: &WorkingDirectoryBinding) -> bool {
    let Ok(root) = binding.root.canonicalize() else {
        return false;
    };
    if !root.is_dir() {
        return false;
    }
    let Ok(source_repository_path) = binding.source_repository_path.canonicalize() else {
        return false;
    };
    source_repository_path.is_dir()
}

fn binding_current_revision(binding: &WorkingDirectoryBinding) -> (Option<String>, Option<String>) {
    let current_ref = git_stdout(binding.root(), ["rev-parse", "HEAD"])
        .ok()
        .filter(|value| !value.is_empty());
    if current_ref.is_none() {
        return (None, None);
    }
    let current_selector = git_stdout(
        binding.root(),
        ["symbolic-ref", "--short", "--quiet", "HEAD"],
    )
    .ok()
    .filter(|value| !value.is_empty());
    (current_selector, current_ref)
}

fn binding_cleanliness(binding: &WorkingDirectoryBinding) -> String {
    match git_stdout(binding.root(), ["status", "--porcelain"]) {
        Ok(output) if output.is_empty() => "clean".to_string(),
        Ok(_) => "dirty".to_string(),
        Err(_) => "unknown".to_string(),
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeGitCacheMaterializer {
    runtime_root: PathBuf,
    repository_access: Arc<Mutex<HashMap<String, RepositorySshMaterializationAccess>>>,
    cache_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl RuntimeGitCacheMaterializer {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            repository_access: Arc::new(Mutex::new(HashMap::new())),
            cache_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    fn working_directory_id(_worker_ref: &WorkerRef, repository_id: &str) -> String {
        next_working_directory_id(repository_id)
    }

    fn working_directory_root(&self, working_directory_id: &str) -> PathBuf {
        self.runtime_root.join(working_directory_id)
    }

    fn repository_cache_key(request: &WorkingDirectoryRequest) -> String {
        let mut digest = Sha256::new();
        if let Some(materialization) = &request.materialization {
            digest.update(materialization.workspace_id.as_bytes());
            digest.update([0]);
            digest.update(materialization.cache_generation.to_be_bytes());
        }
        digest.update(request.repository.id.as_bytes());
        digest.update([0]);
        digest.update(request.repository.source.kind.as_str().as_bytes());
        digest.update([0]);
        digest.update(request.repository.source_revision.to_be_bytes());
        digest.update([0]);
        digest.update(request.repository.source_fingerprint.as_bytes());
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn repository_cache_path(&self, request: &WorkingDirectoryRequest) -> PathBuf {
        self.runtime_root
            .join(REPOSITORY_CACHE_DIR)
            .join(format!("{}.git", Self::repository_cache_key(request)))
    }

    fn corrupted_status(&self, working_directory_id: &str) -> WorkingDirectoryStatus {
        WorkingDirectoryStatus {
            summary: WorkingDirectorySummary {
                working_directory_id: working_directory_id.to_string(),
                repository_id: "unknown".to_string(),
                creation_selector: None,
                creation_ref: None,
                current_selector: None,
                current_ref: None,
                materializer_kind: MaterializerKind::RuntimeGitCache,
                cleanup_target: Some(WorkingDirectoryCleanupTarget {
                    kind: "runtime_git_cache_worktree".to_string(),
                    working_directory_id: working_directory_id.to_string(),
                    repository_id: "unknown".to_string(),
                }),
                status: WorkingDirectoryStatusKind::Corrupted,
                cleanliness: Some("unknown".to_string()),
                primary_worker_id: None,
                occupied_by: None,
            },
        }
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
            command_environment: BTreeMap::new(),
            session_resources: Vec::new(),
        })
    }

    fn bind_repository_access(
        &self,
        working_directory_id: &str,
        mut binding: WorkingDirectoryBinding,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        let access = self
            .repository_access
            .lock()
            .map_err(|_| {
                WorkingDirectoryDiagnostic::new(
                    "working_directory_repository_access_unavailable",
                    "Runtime Repository access state is unavailable",
                )
            })?
            .remove(working_directory_id);
        let Some(access) = access else {
            if binding
                .working_directory
                .evidence
                .credential_revision
                .is_some()
            {
                return Err(WorkingDirectoryDiagnostic::new(
                    "working_directory_remote_repository_access_required",
                    "SSH Repository access must be reacquired before opening the Workdir session",
                ));
            }
            return Ok(binding);
        };
        validate_ssh_materialization_access(&access)?;
        let command_access = Arc::new(RepositoryCommandAccess::prepare_ssh(
            &self.runtime_root,
            &format!("attachment-{working_directory_id}"),
            &binding.working_directory.repository_id,
            &access,
        )?);
        let weak_access = Arc::downgrade(&command_access);
        let expires_at = access.expires_at_epoch_seconds;
        std::thread::spawn(move || {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if expires_at > now {
                std::thread::sleep(Duration::from_secs(expires_at - now));
            }
            if let Some(access) = weak_access.upgrade() {
                access.stop();
            }
        });
        binding.command_environment.insert(
            "SSH_AUTH_SOCK".to_string(),
            command_access.agent.socket.to_string_lossy().to_string(),
        );
        binding.command_environment.insert(
            "GIT_SSH_COMMAND".to_string(),
            command_access.ssh_command.to_string_lossy().to_string(),
        );
        binding.command_environment.insert(
            "YOI_REPOSITORY_ACCESS".to_string(),
            match access.access {
                workspace_api::RepositoryAccessMode::ReadOnly => "read_only",
                workspace_api::RepositoryAccessMode::ReadWrite => "read_write",
            }
            .to_string(),
        );
        binding.session_resources.push(command_access);
        Ok(binding)
    }

    fn validate_request(
        request: &WorkingDirectoryRequest,
    ) -> Result<(), WorkingDirectoryDiagnostic> {
        if !matches!(
            request.materializer,
            MaterializerKind::RuntimeGitCache | MaterializerKind::LocalGitWorktree
        ) {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_materializer_unsupported",
                "the requested working directory materializer is unsupported",
            ));
        }
        if request.repository.provider != "git" {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_repository_provider_unsupported",
                "the configured Repository provider is unsupported",
            ));
        }
        if matches!(
            request.repository.source.kind,
            workspace_api::RepositorySourceKind::Https
                | workspace_api::RepositorySourceKind::Http
                | workspace_api::RepositorySourceKind::Ssh
        ) {
            validate_remote_source_uri(request)?;
            let materialization = request.materialization.as_ref().ok_or_else(|| {
                WorkingDirectoryDiagnostic::new(
                    "working_directory_repository_materialization_authority_required",
                    "remote Repository materialization requires Backend-authored operation authority",
                )
            })?;
            if materialization.workspace_id.trim().is_empty()
                || materialization.runtime_id.trim().is_empty()
                || materialization.operation_id.trim().is_empty()
                || materialization.config_revision == 0
                || materialization.config_projection_digest.trim().is_empty()
            {
                return Err(WorkingDirectoryDiagnostic::new(
                    "working_directory_repository_materialization_authority_invalid",
                    "remote Repository materialization authority is invalid",
                ));
            }
        }
        match request.repository.source.kind {
            workspace_api::RepositorySourceKind::LocalPath
            | workspace_api::RepositorySourceKind::File
            | workspace_api::RepositorySourceKind::Https
            | workspace_api::RepositorySourceKind::Http => {}
            workspace_api::RepositorySourceKind::Ssh => {
                let ssh = request
                    .materialization
                    .as_ref()
                    .and_then(|materialization| materialization.ssh.as_ref())
                    .ok_or_else(|| {
                        WorkingDirectoryDiagnostic::new(
                            "working_directory_remote_repository_access_required",
                            "SSH Repository materialization requires operation-scoped credential and host-trust authority",
                        )
                    })?;
                validate_ssh_materialization_access(ssh)?;
            }
            workspace_api::RepositorySourceKind::Invalid => {
                return Err(WorkingDirectoryDiagnostic::new(
                    "working_directory_repository_source_invalid",
                    "configured Repository source is invalid and cannot be materialized",
                ));
            }
        }
        validate_selector(request.repository.selector.as_deref().unwrap_or("HEAD"))
    }

    fn ensure_repository_cache(
        &self,
        request: &WorkingDirectoryRequest,
    ) -> Result<PathBuf, WorkingDirectoryDiagnostic> {
        Self::validate_request(request)?;
        let cache_key = Self::repository_cache_key(request);
        let cache_lock = self
            .cache_locks
            .lock()
            .map_err(|_| {
                WorkingDirectoryDiagnostic::new(
                    "working_directory_repository_cache_unavailable",
                    "Runtime Repository cache coordination is unavailable",
                )
            })?
            .entry(cache_key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _cache_guard = cache_lock.lock().map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_repository_cache_unavailable",
                "Runtime Repository cache coordination is unavailable",
            )
        })?;
        let cache_path = self.repository_cache_path(request);
        let cache_parent = cache_path.parent().ok_or_else(|| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_repository_cache_invalid",
                "Runtime Repository cache path is invalid",
            )
        })?;
        fs::create_dir_all(cache_parent).map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_repository_cache_create_failed",
                "Runtime Repository cache could not be created; backend-private path details were omitted",
            )
        })?;

        let access = RepositoryCommandAccess::prepare(&self.runtime_root, request)?;
        if cache_path.exists() {
            if git_dir_stdout(&cache_path, ["rev-parse", "--is-bare-repository"])? != "true"
                || git_dir_stdout(&cache_path, ["remote", "get-url", "origin"])?
                    != request.repository.source.uri
            {
                return Err(WorkingDirectoryDiagnostic::new(
                    "working_directory_repository_cache_identity_mismatch",
                    "Runtime Repository cache identity does not match the requested source",
                ));
            }
            fetch_repository_cache(request, access.as_ref(), &cache_path)?;
        } else {
            let staging = cache_path.with_extension(format!(
                "staging-{}",
                next_working_directory_id(&request.repository.id)
            ));
            if staging.exists() {
                let _ = fs::remove_dir_all(&staging);
            }
            fs::create_dir_all(&staging).map_err(|_| {
                WorkingDirectoryDiagnostic::new(
                    "working_directory_repository_cache_create_failed",
                    "Runtime Repository cache could not be created; backend-private path details were omitted",
                )
            })?;
            let mut init = isolated_git_command();
            init.args(["init", "--bare"]).arg(&staging);
            let mut add_origin = repository_git_command(request, access.as_ref());
            add_origin
                .arg("--git-dir")
                .arg(&staging)
                .args(["remote", "add", "origin"])
                .arg(&request.repository.source.uri);
            let mut configure_fetch = isolated_git_command();
            configure_fetch.arg("--git-dir").arg(&staging).args([
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ]);
            let initialized =
                run_repository_git(init, "working_directory_repository_cache_create_failed")
                    .and_then(|_| {
                        run_repository_git(
                            add_origin,
                            "working_directory_repository_cache_create_failed",
                        )
                    })
                    .and_then(|_| {
                        run_repository_git(
                            configure_fetch,
                            "working_directory_repository_cache_create_failed",
                        )
                    })
                    .and_then(|_| fetch_repository_cache(request, access.as_ref(), &staging));
            if let Err(error) = initialized {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
            if let Err(error) = fs::rename(&staging, &cache_path) {
                let _ = fs::remove_dir_all(&staging);
                if !cache_path.exists() {
                    return Err(WorkingDirectoryDiagnostic::new(
                        "working_directory_repository_cache_publish_failed",
                        format!(
                            "Runtime Repository cache could not be published: {}",
                            error.kind()
                        ),
                    ));
                }
            }
        }
        validate_repository_cache_limits(&cache_path)?;
        Ok(cache_path)
    }

    fn materialize_with_working_directory_id(
        &self,
        working_directory_id: String,
        request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        validate_working_directory_id(&working_directory_id)?;
        let repository_cache = self.ensure_repository_cache(request)?;
        let selector = request.repository.selector.as_deref().unwrap_or("HEAD");
        let resolved_commit = resolve_cached_commit(&repository_cache, selector)?;
        let tree_spec = format!("{resolved_commit}^{{tree}}");
        let resolved_tree = git_dir_stdout(&repository_cache, ["rev-parse", tree_spec.as_str()])
            .ok()
            .filter(|value| !value.is_empty());

        let working_directory_root = self.working_directory_root(&working_directory_id);
        let worktree_root = working_directory_root.join(CHECKOUT_DIR);
        if worktree_root.exists() {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_exists",
                "working directory target already exists; cleanup or choose a new working_directory",
            ));
        }
        fs::create_dir_all(&working_directory_root).map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_create_failed",
                "failed to create working directory; backend-private path details were omitted",
            )
        })?;
        let mut command = isolated_git_command();
        command
            .arg("--git-dir")
            .arg(&repository_cache)
            .args(["worktree", "add", "--detach"])
            .arg(&worktree_root)
            .arg(&resolved_commit);
        if let Err(error) = run_repository_git(command, "working_directory_git_failed") {
            let _ = fs::remove_dir_all(&working_directory_root);
            return Err(error);
        }
        if request
            .materialization
            .as_ref()
            .and_then(|materialization| materialization.ssh.as_ref())
            .is_some_and(|ssh| ssh.access == workspace_api::RepositoryAccessMode::ReadOnly)
        {
            let mut enable_worktree_config = isolated_git_command();
            enable_worktree_config
                .arg("--git-dir")
                .arg(&repository_cache)
                .args(["config", "extensions.worktreeConfig", "true"]);
            let mut disable_push = isolated_git_command();
            disable_push.arg("-C").arg(&worktree_root).args([
                "config",
                "--worktree",
                "remote.origin.pushurl",
                "yoi-read-only://repository-push-disabled",
            ]);
            if let Err(error) = run_repository_git(
                enable_worktree_config,
                "working_directory_repository_policy_failed",
            )
            .and_then(|_| {
                run_repository_git(disable_push, "working_directory_repository_policy_failed")
            }) {
                remove_cached_worktree(&repository_cache, &worktree_root);
                let _ = fs::remove_dir_all(&working_directory_root);
                return Err(error);
            }
        }

        let context = request.materialization.as_ref();
        let working_directory = WorkingDirectory {
            id: working_directory_id.clone(),
            repository_id: request.repository.id.clone(),
            materializer_kind: MaterializerKind::RuntimeGitCache,
            evidence: WorkingDirectoryEvidence {
                repository_id: request.repository.id.clone(),
                requested_selector: request
                    .repository
                    .selector
                    .as_ref()
                    .map(|selector| selector.as_ref().to_string()),
                resolved_commit,
                resolved_tree,
                materializer_kind: MaterializerKind::RuntimeGitCache,
                repository_source_revision: Some(request.repository.source_revision),
                repository_source_fingerprint: Some(request.repository.source_fingerprint.clone()),
                repository_cache_key: Some(Self::repository_cache_key(request)),
                cache_generation: context
                    .map(|value| value.cache_generation)
                    .unwrap_or_default(),
                operation_id: context.map(|value| value.operation_id.clone()),
                credential_revision: context
                    .and_then(|value| value.ssh.as_ref())
                    .map(|value| value.credential_revision),
                host_trust_revision: context
                    .and_then(|value| value.ssh.as_ref())
                    .map(|value| value.host_trust_revision),
                transport_warning: repository_transport_warning(request.repository.source.kind)
                    .map(str::to_string),
            },
            cleanup_target: WorkingDirectoryCleanupTarget {
                kind: "runtime_git_cache_worktree".to_string(),
                working_directory_id,
                repository_id: request.repository.id.clone(),
            },
            status: WorkingDirectoryStatusKind::Active,
        };
        let binding = WorkingDirectoryBinding {
            working_directory,
            root: worktree_root.clone(),
            cwd: worktree_root.clone(),
            working_directory_root: working_directory_root.clone(),
            source_repository_path: repository_cache.clone(),
            command_environment: BTreeMap::new(),
            session_resources: Vec::new(),
        };
        if let Err(error) = self.write_record(&binding) {
            remove_cached_worktree(&repository_cache, &worktree_root);
            let _ = fs::remove_dir_all(&working_directory_root);
            return Err(error);
        }
        Ok(binding)
    }
}

impl WorkingDirectoryMaterializer for RuntimeGitCacheMaterializer {
    fn materialize(
        &self,
        worker_ref: &WorkerRef,
        request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        let working_directory_id = Self::working_directory_id(worker_ref, &request.repository.id);
        self.materialize_with_working_directory_id(working_directory_id, request)
    }

    fn create(
        &self,
        request: &WorkingDirectoryRequest,
    ) -> Result<WorkingDirectoryBinding, WorkingDirectoryDiagnostic> {
        let working_directory_id = request
            .backend_workdir_id
            .clone()
            .unwrap_or_else(|| next_working_directory_id(&request.repository.id));
        self.materialize_with_working_directory_id(working_directory_id, request)
    }

    fn authorize_repository_access(
        &self,
        request: &WorkingDirectoryRepositoryAccessRequest,
    ) -> Result<(), WorkingDirectoryDiagnostic> {
        validate_working_directory_id(&request.working_directory_id)?;
        let ssh = request.materialization.ssh.as_ref().ok_or_else(|| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_remote_repository_access_required",
                "SSH Repository access authority is required",
            )
        })?;
        validate_ssh_materialization_access(ssh)?;
        let mut binding = self.read_binding(&request.working_directory_id)?;
        if binding
            .working_directory
            .evidence
            .credential_revision
            .is_none()
        {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_repository_access_not_applicable",
                "Workdir is not backed by an SSH Repository",
            ));
        }
        binding.working_directory.evidence.operation_id =
            Some(request.materialization.operation_id.clone());
        binding.working_directory.evidence.credential_revision = Some(ssh.credential_revision);
        binding.working_directory.evidence.host_trust_revision = Some(ssh.host_trust_revision);
        self.write_record(&binding)?;
        let working_directory_id = request.working_directory_id.clone();
        let credential_revision = ssh.credential_revision;
        let expires_at = ssh.expires_at_epoch_seconds;
        self.repository_access
            .lock()
            .map_err(|_| {
                WorkingDirectoryDiagnostic::new(
                    "working_directory_repository_access_unavailable",
                    "Runtime Repository access state is unavailable",
                )
            })?
            .insert(working_directory_id.clone(), ssh.clone());
        let repository_access = self.repository_access.clone();
        std::thread::spawn(move || {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if expires_at > now {
                std::thread::sleep(Duration::from_secs(expires_at - now));
            }
            if let Ok(mut access) = repository_access.lock()
                && access
                    .get(&working_directory_id)
                    .is_some_and(|access| access.credential_revision == credential_revision)
            {
                access.remove(&working_directory_id);
            }
        });
        Ok(())
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
        let binding = self.bind_repository_access(working_directory_id, binding)?;
        let cwd = validate_relative_cwd(binding.root(), relative_cwd)?;
        Ok(WorkingDirectoryBinding { cwd, ..binding })
    }

    fn list_working_directories(
        &self,
    ) -> Result<Vec<WorkingDirectoryStatus>, WorkingDirectoryDiagnostic> {
        let entries = match fs::read_dir(&self.runtime_root) {
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
            if working_directory_id.starts_with('.')
                || validate_working_directory_id(&working_directory_id).is_err()
            {
                continue;
            }
            match self.read_binding(&working_directory_id) {
                Ok(binding) => statuses.push(binding.status()),
                Err(_) => statuses.push(self.corrupted_status(&working_directory_id)),
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
        let working_directory_root = self.working_directory_root(working_directory_id);
        if !working_directory_root.exists() {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_not_found",
                "working directory working_directory was not found",
            ));
        }
        match self.read_binding(working_directory_id) {
            Ok(binding) => Ok(binding.status()),
            Err(_) => Ok(self.corrupted_status(working_directory_id)),
        }
    }

    fn cleanup_working_directory(
        &self,
        working_directory_id: &str,
    ) -> Result<WorkingDirectoryStatus, WorkingDirectoryDiagnostic> {
        validate_working_directory_id(working_directory_id)?;
        let status = self.working_directory_status(working_directory_id)?;
        if status.summary.status == WorkingDirectoryStatusKind::Corrupted {
            let working_directory_root = self.working_directory_root(working_directory_id);
            if working_directory_root.exists() {
                fs::remove_dir_all(&working_directory_root).map_err(|_| {
                    WorkingDirectoryDiagnostic::new(
                        "working_directory_corrupted_cleanup_failed",
                        "failed to remove corrupted working directory; backend-private path details were omitted",
                    )
                })?;
            }
            let mut summary = status.summary;
            summary.status = WorkingDirectoryStatusKind::NotFound;
            return Ok(WorkingDirectoryStatus { summary });
        }
        let binding = self.read_binding(working_directory_id)?;
        self.cleanup(&binding)?;
        let mut summary = binding.working_directory.status_summary();
        summary.status = WorkingDirectoryStatusKind::NotFound;
        summary.cleanliness = Some("unknown".to_string());
        if binding.working_directory_root.exists() {
            fs::remove_dir_all(&binding.working_directory_root).map_err(|_| {
                WorkingDirectoryDiagnostic::new(
                    "working_directory_record_cleanup_failed",
                    "failed to remove working directory record; backend-private path details were omitted",
                )
            })?;
        }
        Ok(WorkingDirectoryStatus { summary })
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
        let mut remove_command = isolated_git_command();
        remove_command
            .arg("--git-dir")
            .arg(binding.source_repository_path())
            .args([
                "worktree",
                "remove",
                "--force",
                workspace_worktree_root_arg.as_str(),
            ]);
        let remove_result = run_repository_git(
            remove_command,
            "working_directory_cleanup_failed",
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
        if remove_result.is_err() {
            working_directory.status = WorkingDirectoryStatusKind::CleanupPending;
            let updated = WorkingDirectoryBinding {
                working_directory,
                root: binding.root.clone(),
                cwd: binding.cwd.clone(),
                working_directory_root: binding.working_directory_root.clone(),
                source_repository_path: binding.source_repository_path.clone(),
                command_environment: BTreeMap::new(),
                session_resources: Vec::new(),
            };
            let _ = self.write_record(&updated);
        } else if let Ok(mut access) = self.repository_access.lock() {
            access.remove(&binding.working_directory.id);
        }
        remove_result
    }
}

#[derive(Debug)]
struct RepositorySshAgent {
    root: PathBuf,
    socket: PathBuf,
    child: Mutex<Option<std::process::Child>>,
}

impl RepositorySshAgent {
    fn start(
        runtime_root: &Path,
        working_directory_id: &str,
        access: &RepositorySshMaterializationAccess,
    ) -> Result<Self, WorkingDirectoryDiagnostic> {
        let root = runtime_root.join(".repository-agents").join(format!(
            "{}-{}",
            sanitize_path_component(working_directory_id),
            next_working_directory_id("agent")
        ));
        fs::create_dir_all(&root).map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_repository_agent_failed",
                "Runtime-managed Repository SSH agent could not be created",
            )
        })?;
        set_directory_owner_only(&root)?;
        let socket = root.join("agent.sock");
        let mut child = Command::new("ssh-agent")
            .args(["-D", "-a"])
            .arg(&socket)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| {
                WorkingDirectoryDiagnostic::new(
                    "working_directory_repository_agent_unavailable",
                    "Runtime-managed Repository SSH agent is unavailable",
                )
            })?;
        let started = Instant::now();
        loop {
            if socket.exists() {
                break;
            }
            if matches!(child.try_wait(), Ok(Some(_)) | Err(_))
                || started.elapsed() >= Duration::from_secs(2)
            {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_dir_all(&root);
                return Err(WorkingDirectoryDiagnostic::new(
                    "working_directory_repository_agent_failed",
                    "Runtime-managed Repository SSH agent could not be started",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let agent = Self {
            root,
            socket,
            child: Mutex::new(Some(child)),
        };
        let mut add = match Command::new("ssh-add")
            .arg("-")
            .env("SSH_AUTH_SOCK", &agent.socket)
            .env("SSH_ASKPASS", "/bin/false")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(add) => add,
            Err(_) => {
                drop(agent);
                return Err(WorkingDirectoryDiagnostic::new(
                    "working_directory_repository_agent_unavailable",
                    "Runtime-managed Repository SSH agent is unavailable",
                ));
            }
        };
        let write_result = add.stdin.as_mut().map_or_else(
            || Err(std::io::Error::other("ssh-add stdin unavailable")),
            |stdin| stdin.write_all(access.private_key.expose().as_bytes()),
        );
        let status = add.wait();
        if write_result.is_err() || !matches!(status, Ok(status) if status.success()) {
            drop(agent);
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_repository_agent_failed",
                "Runtime-managed Repository SSH agent rejected credential material",
            ));
        }
        Ok(agent)
    }

    fn stop(&self) {
        if let Ok(mut child) = self.child.lock()
            && let Some(mut child) = child.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Drop for RepositorySshAgent {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Debug)]
struct RepositoryCommandAccess {
    root: PathBuf,
    ssh_command: PathBuf,
    agent: RepositorySshAgent,
}

impl RepositoryCommandAccess {
    fn prepare(
        runtime_root: &Path,
        request: &WorkingDirectoryRequest,
    ) -> Result<Option<Self>, WorkingDirectoryDiagnostic> {
        if request.repository.source.kind != workspace_api::RepositorySourceKind::Ssh {
            return Ok(None);
        }
        let Some(ssh) = request
            .materialization
            .as_ref()
            .and_then(|materialization| materialization.ssh.as_ref())
        else {
            return Ok(None);
        };
        let operation_id = request
            .materialization
            .as_ref()
            .map(|materialization| materialization.operation_id.as_str())
            .unwrap_or("operation");
        Ok(Some(Self::prepare_ssh(
            runtime_root,
            operation_id,
            &request.repository.id,
            ssh,
        )?))
    }

    fn prepare_ssh(
        runtime_root: &Path,
        operation_id: &str,
        repository_id: &str,
        ssh: &RepositorySshMaterializationAccess,
    ) -> Result<Self, WorkingDirectoryDiagnostic> {
        let root = runtime_root.join(REPOSITORY_ACCESS_DIR).join(format!(
            "{}-{}",
            sanitize_path_component(operation_id),
            next_working_directory_id(repository_id)
        ));
        fs::create_dir_all(&root).map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_repository_access_setup_failed",
                "operation-scoped Repository access could not be prepared",
            )
        })?;
        set_directory_owner_only(&root)?;
        let known_hosts = root.join("known_hosts");
        let ssh_command = root.join("ssh-command");
        write_owner_only(&known_hosts, ssh.known_hosts_entry.expose().as_bytes())?;
        let script = format!(
            "#!/bin/sh\nexec ssh -F /dev/null -o BatchMode=yes -o IdentitiesOnly=no -o IdentityFile=/dev/null -o StrictHostKeyChecking=yes -o UserKnownHostsFile={} \"$@\"\n",
            shell_quote_path(&known_hosts)?,
        );
        write_owner_only(&ssh_command, script.as_bytes())?;
        set_file_owner_executable(&ssh_command)?;
        let agent = RepositorySshAgent::start(runtime_root, operation_id, ssh)?;
        Ok(Self {
            root,
            ssh_command,
            agent,
        })
    }

    fn stop(&self) {
        self.agent.stop();
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Drop for RepositoryCommandAccess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn validate_ssh_materialization_access(
    access: &RepositorySshMaterializationAccess,
) -> Result<(), WorkingDirectoryDiagnostic> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if access.expires_at_epoch_seconds <= now {
        return Err(WorkingDirectoryDiagnostic::new(
            "working_directory_repository_access_expired",
            "operation-scoped SSH credential and host-trust authority has expired",
        ));
    }
    if access.credential_id.trim().is_empty()
        || access.credential_revision == 0
        || access.host_trust_id.trim().is_empty()
        || access.host_trust_revision == 0
        || !access.private_key.expose().contains("PRIVATE KEY")
        || access.known_hosts_entry.expose().trim().is_empty()
    {
        return Err(WorkingDirectoryDiagnostic::new(
            "working_directory_remote_repository_access_invalid",
            "operation-scoped SSH credential or host-trust authority is invalid",
        ));
    }
    Ok(())
}

fn repository_transport_warning(kind: workspace_api::RepositorySourceKind) -> Option<&'static str> {
    (kind == workspace_api::RepositorySourceKind::Http).then_some("plain_http_transport")
}

fn validate_remote_source_uri(
    request: &WorkingDirectoryRequest,
) -> Result<(), WorkingDirectoryDiagnostic> {
    let url = url::Url::parse(&request.repository.source.uri).map_err(|_| {
        WorkingDirectoryDiagnostic::new(
            "working_directory_repository_source_invalid",
            "remote Repository source URI is invalid",
        )
    })?;
    let expected_scheme = match request.repository.source.kind {
        workspace_api::RepositorySourceKind::Https => "https",
        workspace_api::RepositorySourceKind::Http => "http",
        workspace_api::RepositorySourceKind::Ssh => "ssh",
        _ => return Ok(()),
    };
    if url.scheme() != expected_scheme
        || url.host_str().is_none()
        || url.password().is_some()
        || !url.query().is_none()
        || !url.fragment().is_none()
        || (matches!(expected_scheme, "https" | "http") && !url.username().is_empty())
    {
        return Err(WorkingDirectoryDiagnostic::new(
            "working_directory_repository_source_invalid",
            "remote Repository source URI is invalid or contains forbidden credentials",
        ));
    }
    if expected_scheme == "ssh" {
        let access = request
            .materialization
            .as_ref()
            .and_then(|materialization| materialization.ssh.as_ref())
            .ok_or_else(|| {
                WorkingDirectoryDiagnostic::new(
                    "working_directory_remote_repository_access_required",
                    "SSH Repository materialization requires operation-scoped credential and host-trust authority",
                )
            })?;
        let host = url.host_str().unwrap_or_default();
        let known_host = if url.port().unwrap_or(22) == 22 {
            format!("{host} ")
        } else {
            format!("[{host}]:{} ", url.port().unwrap_or(22))
        };
        if !access.known_hosts_entry.expose().starts_with(&known_host) {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_remote_repository_host_trust_mismatch",
                "SSH Repository source does not match the operation-scoped host-trust authority",
            ));
        }
    }
    Ok(())
}

fn validate_selector(selector: &str) -> Result<(), WorkingDirectoryDiagnostic> {
    let valid = !selector.is_empty()
        && selector.len() <= 512
        && !selector.starts_with('-')
        && !selector.ends_with('.')
        && !selector.contains("..")
        && !selector.contains("@{")
        && !selector.contains("//")
        && !selector.chars().any(|ch| {
            ch.is_control()
                || ch.is_whitespace()
                || matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        });
    if !valid {
        return Err(WorkingDirectoryDiagnostic::new(
            "working_directory_repository_selector_invalid",
            "configured Repository selector is invalid",
        ));
    }
    Ok(())
}

fn isolated_git_command() -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ASKPASS", "/bin/false")
        .env("SSH_ASKPASS", "/bin/false")
        .env("LC_ALL", "C")
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "core.askPass=/bin/false",
            "-c",
            "http.followRedirects=false",
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "submodule.recurse=false",
        ]);
    command
}

fn repository_git_command(
    request: &WorkingDirectoryRequest,
    access: Option<&RepositoryCommandAccess>,
) -> Command {
    let mut command = isolated_git_command();
    let file_policy = if matches!(
        request.repository.source.kind,
        workspace_api::RepositorySourceKind::LocalPath | workspace_api::RepositorySourceKind::File
    ) {
        "always"
    } else {
        "never"
    };
    command.args(["-c", &format!("protocol.file.allow={file_policy}")]);
    if let Some(access) = access {
        command
            .env("GIT_SSH_COMMAND", &access.ssh_command)
            .env("SSH_AUTH_SOCK", &access.agent.socket);
    }
    command
}

fn run_repository_git(
    mut command: Command,
    code: &'static str,
) -> Result<(), WorkingDirectoryDiagnostic> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_git_unavailable",
                "Git command could not be executed; backend-private path details were omitted",
            )
        })?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                code,
                "Git Repository operation failed; credentials and backend-private path details were omitted",
            )
        })? {
            break status;
        }
        if started.elapsed() >= REPOSITORY_COMMAND_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_repository_timeout",
                "Git Repository operation exceeded the Runtime time limit",
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if status.success() {
        Ok(())
    } else {
        Err(WorkingDirectoryDiagnostic::new(
            code,
            "Git Repository operation failed; credentials and backend-private path details were omitted",
        ))
    }
}

fn resolve_cached_commit(
    repository_cache: &Path,
    selector: &str,
) -> Result<String, WorkingDirectoryDiagnostic> {
    let mut candidates = Vec::new();
    if selector == "HEAD" {
        candidates.push("FETCH_HEAD".to_string());
    } else if let Some(branch) = selector.strip_prefix("refs/heads/") {
        candidates.push(format!("refs/remotes/origin/{branch}"));
    } else if selector.starts_with("refs/") || selector.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        candidates.push(selector.to_string());
    } else {
        candidates.push(format!("refs/remotes/origin/{selector}"));
        candidates.push(selector.to_string());
    }
    for candidate in candidates {
        let spec = format!("{candidate}^{{commit}}");
        if let Ok(commit) = git_dir_stdout(repository_cache, ["rev-parse", spec.as_str()])
            && !commit.is_empty()
        {
            return Ok(commit);
        }
    }
    Err(WorkingDirectoryDiagnostic::new(
        "working_directory_repository_selector_unresolved",
        "configured Repository selector could not be resolved to a commit",
    ))
}

fn fetch_repository_cache(
    request: &WorkingDirectoryRequest,
    access: Option<&RepositoryCommandAccess>,
    repository_cache: &Path,
) -> Result<(), WorkingDirectoryDiagnostic> {
    let mut refs = repository_git_command(request, access);
    refs.arg("--git-dir").arg(repository_cache).args([
        "fetch",
        "--prune",
        "--tags",
        "origin",
        "+refs/heads/*:refs/remotes/origin/*",
    ]);
    let mut head = repository_git_command(request, access);
    head.arg("--git-dir")
        .arg(repository_cache)
        .args(["fetch", "--no-tags", "origin", "HEAD"]);
    run_repository_git(refs, "working_directory_repository_fetch_failed")?;
    run_repository_git(head, "working_directory_repository_fetch_failed")
}

fn validate_repository_cache_limits(
    repository_cache: &Path,
) -> Result<(), WorkingDirectoryDiagnostic> {
    let report = git_dir_stdout(repository_cache, ["count-objects", "-v"])?;
    let mut objects = 0u64;
    let mut kibibytes = 0u64;
    for line in report.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().parse::<u64>().unwrap_or(u64::MAX);
        match key {
            "count" | "in-pack" | "garbage" => objects = objects.saturating_add(value),
            "size" | "size-pack" | "size-garbage" => kibibytes = kibibytes.saturating_add(value),
            _ => {}
        }
    }
    if objects > REPOSITORY_MAX_OBJECTS || kibibytes.saturating_mul(1024) > REPOSITORY_MAX_BYTES {
        return Err(WorkingDirectoryDiagnostic::new(
            "working_directory_repository_limit_exceeded",
            "Git Repository exceeds Runtime object or storage limits",
        ));
    }
    Ok(())
}

fn remove_cached_worktree(repository_cache: &Path, worktree_root: &Path) {
    let mut command = isolated_git_command();
    command
        .arg("--git-dir")
        .arg(repository_cache)
        .args(["worktree", "remove", "--force"])
        .arg(worktree_root);
    let _ = run_repository_git(command, "working_directory_cleanup_failed");
}

fn git_dir_stdout<'a, I>(
    repository_path: &Path,
    args: I,
) -> Result<String, WorkingDirectoryDiagnostic>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut command = isolated_git_command();
    let output = command
        .arg("--git-dir")
        .arg(repository_path)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_git_unavailable",
                "Git command could not be executed; backend-private path details were omitted",
            )
        })?;
    if !output.status.success() || output.stdout.len() > 4096 {
        return Err(WorkingDirectoryDiagnostic::new(
            "working_directory_repository_selector_unresolved",
            "configured Repository selector could not be resolved to a commit",
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn write_owner_only(path: &Path, content: &[u8]) -> Result<(), WorkingDirectoryDiagnostic> {
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| {
        WorkingDirectoryDiagnostic::new(
            "working_directory_repository_access_setup_failed",
            "operation-scoped Repository access could not be prepared",
        )
    })?;
    file.write_all(content).map_err(|_| {
        WorkingDirectoryDiagnostic::new(
            "working_directory_repository_access_setup_failed",
            "operation-scoped Repository access could not be prepared",
        )
    })
}

fn set_directory_owner_only(path: &Path) -> Result<(), WorkingDirectoryDiagnostic> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_repository_access_setup_failed",
                "operation-scoped Repository access could not be prepared",
            )
        })?;
    }
    Ok(())
}

fn set_file_owner_executable(path: &Path) -> Result<(), WorkingDirectoryDiagnostic> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| {
            WorkingDirectoryDiagnostic::new(
                "working_directory_repository_access_setup_failed",
                "operation-scoped Repository access could not be prepared",
            )
        })?;
    }
    Ok(())
}

fn shell_quote_path(path: &Path) -> Result<String, WorkingDirectoryDiagnostic> {
    let value = path_str(path)?;
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
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

fn path_str(path: &Path) -> Result<String, WorkingDirectoryDiagnostic> {
    path.to_str().map(ToString::to_string).ok_or_else(|| {
        WorkingDirectoryDiagnostic::new(
            "working_directory_non_utf8_path",
            "working directory path is not valid UTF-8; backend-private path details were omitted",
        )
    })
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

fn next_working_directory_id(_repository_id: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default();
    let sequence = NEXT_WORKING_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed) & 0x00ff_ffff;
    format!("{now:013x}{sequence:06x}")
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
    use crate::identity::{WorkerId, WorkerRef};

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
                source: workspace_api::RepositorySource {
                    kind: workspace_api::RepositorySourceKind::LocalPath,
                    uri: repo.display().to_string(),
                },
                source_revision: 1,
                source_fingerprint: "sha256:test".to_string(),
                selector: Some(RepositorySelector::from("HEAD")),
            },
            materializer: MaterializerKind::RuntimeGitCache,
            backend_workdir_id: None,
            materialization: None,
        }
    }

    fn worker_ref(sequence: u64) -> WorkerRef {
        WorkerRef::new(WorkerId::from_legacy_u64(sequence))
    }

    #[test]
    fn local_git_repo_materializes_detached_worktree_under_runtime_root() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
        let binding = materializer
            .materialize(&worker_ref(1), &request(repo.path()))
            .unwrap();

        assert!(binding.root.starts_with(runtime_root.path()));
        assert_eq!(
            binding.working_directory_root(),
            runtime_root
                .path()
                .join(&binding.working_directory.id)
                .as_path()
        );
        assert_eq!(
            binding.root(),
            binding.working_directory_root().join(CHECKOUT_DIR)
        );
        assert!(binding.root.join("README.md").exists());
        let branch = git_stdout(binding.root(), ["branch", "--show-current"]).unwrap();
        assert!(
            branch.is_empty(),
            "worktree should be detached, got {branch}"
        );
        assert_eq!(
            binding.working_directory.materializer_kind,
            MaterializerKind::RuntimeGitCache
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
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
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
    fn dirty_source_is_ignored_by_commit_only_materialization() {
        let repo = create_clean_repo();
        fs::write(repo.path().join("dirty.txt"), "dirty\n").unwrap();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());

        let binding = materializer
            .materialize(&worker_ref(1), &request(repo.path()))
            .unwrap();

        assert!(binding.root.join("README.md").exists());
        assert!(!binding.root.join("dirty.txt").exists());
    }

    #[test]
    fn branch_selector_allows_dirty_source_materialization() {
        let repo = create_clean_repo();
        git(repo.path(), &["branch", "pinned"]);
        fs::write(repo.path().join("dirty.txt"), "dirty\n").unwrap();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
        let mut request = request(repo.path());
        request.repository.selector = Some(RepositorySelector::from("pinned"));

        let binding = materializer.materialize(&worker_ref(1), &request).unwrap();

        assert_eq!(
            binding
                .working_directory
                .evidence
                .requested_selector
                .as_deref(),
            Some("pinned")
        );
        assert!(binding.root.join("README.md").exists());
        assert!(!binding.root.join("dirty.txt").exists());
    }

    #[test]
    fn file_and_local_sources_share_the_runtime_cache_pipeline() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
        let local = materializer
            .materialize(&worker_ref(1), &request(repo.path()))
            .unwrap();
        let second = materializer
            .materialize(&worker_ref(2), &request(repo.path()))
            .unwrap();

        assert_eq!(
            local.working_directory.evidence.repository_cache_key,
            second.working_directory.evidence.repository_cache_key
        );
        assert_eq!(
            git_dir_stdout(
                local.source_repository_path(),
                ["config", "--get-all", "remote.origin.fetch"],
            )
            .unwrap(),
            "+refs/heads/*:refs/remotes/origin/*"
        );
        assert!(
            git_dir_stdout(
                local.source_repository_path(),
                ["config", "--get", "remote.origin.mirror"],
            )
            .is_err()
        );
        assert_eq!(
            fs::read_dir(runtime_root.path().join(REPOSITORY_CACHE_DIR))
                .unwrap()
                .count(),
            1
        );

        let mut file_request = request(repo.path());
        file_request.repository.source.kind = workspace_api::RepositorySourceKind::File;
        file_request.repository.source.uri = format!("file://{}", repo.path().display());
        file_request.repository.source_revision = 2;
        file_request.repository.source_fingerprint = "sha256:file-source".to_string();
        let file = materializer
            .materialize(&worker_ref(3), &file_request)
            .unwrap();
        assert!(file.root.join("README.md").exists());
        assert_ne!(
            local.working_directory.evidence.repository_cache_key,
            file.working_directory.evidence.repository_cache_key
        );
    }

    #[test]
    fn materialization_context_is_audited_without_secret_values() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
        let mut request = request(repo.path());
        request.materialization = Some(crate::catalog::RepositoryMaterializationContext {
            workspace_id: "workspace-1".to_string(),
            runtime_id: "runtime-1".to_string(),
            operation_id: "operation-1".to_string(),
            config_revision: 7,
            config_projection_digest: "sha256:projection".to_string(),
            cache_generation: 3,
            ssh: None,
        });

        let binding = materializer.materialize(&worker_ref(1), &request).unwrap();
        assert_eq!(
            binding.working_directory.evidence.operation_id.as_deref(),
            Some("operation-1")
        );
        assert_eq!(binding.working_directory.evidence.cache_generation, 3);
        let record = fs::read_to_string(
            binding
                .working_directory_root()
                .join(MATERIALIZATION_RECORD),
        )
        .unwrap();
        assert!(!record.contains("private key"));
    }

    #[test]
    fn sensitive_repository_access_debug_output_is_redacted() {
        let access = crate::catalog::RepositorySshMaterializationAccess {
            credential_id: "credential-1".to_string(),
            credential_revision: 2,
            host_trust_id: "trust-1".to_string(),
            host_trust_revision: 4,
            access: workspace_api::RepositoryAccessMode::ReadOnly,
            expires_at_epoch_seconds: u64::MAX,
            private_key: crate::catalog::SensitiveString::new("PRIVATE KEY secret bytes"),
            known_hosts_entry: crate::catalog::SensitiveString::new("host key secret bytes"),
        };

        let debug = format!("{access:?}");
        assert!(!debug.contains("secret bytes"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn bound_ssh_workdir_uses_attachment_scoped_agent_and_cleans_it_up() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let key_root = tempfile::tempdir().unwrap();
        let key_path = key_root.path().join("id_ed25519");
        let status = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key_path)
            .status()
            .unwrap();
        assert!(status.success());
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
        let mut request = request(repo.path());
        request.materialization = Some(crate::catalog::RepositoryMaterializationContext {
            workspace_id: "workspace-1".to_string(),
            runtime_id: "runtime-1".to_string(),
            operation_id: "operation-agent".to_string(),
            config_revision: 2,
            config_projection_digest: "sha256:projection".to_string(),
            cache_generation: 0,
            ssh: Some(crate::catalog::RepositorySshMaterializationAccess {
                credential_id: "credential-1".to_string(),
                credential_revision: 1,
                host_trust_id: "trust-1".to_string(),
                host_trust_revision: 1,
                access: workspace_api::RepositoryAccessMode::ReadWrite,
                expires_at_epoch_seconds: u64::MAX,
                private_key: crate::catalog::SensitiveString::new(
                    fs::read_to_string(&key_path).unwrap(),
                ),
                known_hosts_entry: crate::catalog::SensitiveString::new(
                    "example.test ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIexample",
                ),
            }),
        });
        let mut ssh_operation = request.clone();
        ssh_operation.repository.source = workspace_api::RepositorySource {
            kind: workspace_api::RepositorySourceKind::Ssh,
            uri: "ssh://git@example.test/repo.git".to_string(),
        };
        let command_access = RepositoryCommandAccess::prepare(runtime_root.path(), &ssh_operation)
            .unwrap()
            .unwrap();
        assert!(!command_access.root.join("identity").exists());
        assert!(command_access.agent.socket.exists());
        let operation_socket = command_access.agent.socket.clone();
        drop(command_access);
        assert!(!operation_socket.exists());

        let created = materializer.create(&request).unwrap();
        let id = created.working_directory.id;
        assert_eq!(materializer.list_working_directories().unwrap().len(), 1);
        assert_eq!(
            fs::read_dir(runtime_root.path().join(".repository-agents"))
                .map(|entries| entries.count())
                .unwrap_or_default(),
            0
        );
        materializer
            .authorize_repository_access(&WorkingDirectoryRepositoryAccessRequest {
                working_directory_id: id.clone(),
                materialization: request.materialization.clone().unwrap(),
            })
            .unwrap();
        let binding = materializer.bind_working_directory(&id, None).unwrap();
        let environment = binding.command_environment();
        let socket = PathBuf::from(environment["SSH_AUTH_SOCK"].clone());
        let ssh_command = PathBuf::from(environment["GIT_SSH_COMMAND"].clone());
        assert!(socket.exists());
        assert!(ssh_command.exists());
        let ssh_policy = fs::read_to_string(&ssh_command).unwrap();
        assert!(ssh_policy.contains("StrictHostKeyChecking=yes"));
        assert!(ssh_policy.contains("UserKnownHostsFile="));
        assert_eq!(environment["YOI_REPOSITORY_ACCESS"], "read_write");
        drop(binding);
        assert!(!socket.exists());
        assert!(!ssh_command.exists());
        assert_eq!(
            materializer
                .bind_working_directory(&id, None)
                .unwrap_err()
                .code,
            "working_directory_remote_repository_access_required"
        );
        let mut rotated = request.materialization.clone().unwrap();
        rotated.operation_id = "operation-agent-rotated".to_string();
        rotated.ssh.as_mut().unwrap().credential_revision = 2;
        materializer
            .authorize_repository_access(&WorkingDirectoryRepositoryAccessRequest {
                working_directory_id: id.clone(),
                materialization: rotated,
            })
            .unwrap();
        let rebound = materializer.bind_working_directory(&id, None).unwrap();
        assert_eq!(
            rebound.working_directory.evidence.credential_revision,
            Some(2)
        );
        drop(rebound);
        let mut expired = request.materialization.clone().unwrap();
        expired.ssh.as_mut().unwrap().expires_at_epoch_seconds = 1;
        assert_eq!(
            materializer
                .authorize_repository_access(&WorkingDirectoryRepositoryAccessRequest {
                    working_directory_id: id.clone(),
                    materialization: expired,
                })
                .unwrap_err()
                .code,
            "working_directory_repository_access_expired"
        );
        let restored = RuntimeGitCacheMaterializer::new(runtime_root.path());
        assert_eq!(
            restored.bind_working_directory(&id, None).unwrap_err().code,
            "working_directory_remote_repository_access_required"
        );
    }

    #[test]
    fn read_only_repository_access_disables_default_push_target() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
        let mut request = request(repo.path());
        request.materialization = Some(crate::catalog::RepositoryMaterializationContext {
            workspace_id: "workspace-1".to_string(),
            runtime_id: "runtime-1".to_string(),
            operation_id: "operation-read-only".to_string(),
            config_revision: 2,
            config_projection_digest: "sha256:projection".to_string(),
            cache_generation: 0,
            ssh: Some(crate::catalog::RepositorySshMaterializationAccess {
                credential_id: "credential-1".to_string(),
                credential_revision: 1,
                host_trust_id: "trust-1".to_string(),
                host_trust_revision: 1,
                access: workspace_api::RepositoryAccessMode::ReadOnly,
                expires_at_epoch_seconds: u64::MAX,
                private_key: crate::catalog::SensitiveString::new("PRIVATE KEY placeholder"),
                known_hosts_entry: crate::catalog::SensitiveString::new(
                    "example.test ssh-ed25519 placeholder",
                ),
            }),
        });

        let binding = materializer.create(&request).unwrap();
        assert_eq!(
            git_stdout(
                binding.root(),
                ["config", "--worktree", "--get", "remote.origin.pushurl"],
            )
            .unwrap(),
            "yoi-read-only://repository-push-disabled"
        );
    }

    #[test]
    fn selector_is_not_accepted_as_a_git_option_or_refspec() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
        for selector in ["--upload-pack=evil", "refs/heads/main:evil", "main@{1}"] {
            let mut request = request(repo.path());
            request.repository.selector = Some(RepositorySelector::from(selector));
            assert_eq!(
                materializer
                    .materialize(&worker_ref(1), &request)
                    .unwrap_err()
                    .code,
                "working_directory_repository_selector_invalid"
            );
        }
    }

    #[test]
    fn remote_source_rejects_uri_credentials_and_mismatched_host_trust() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
        let context = |ssh| crate::catalog::RepositoryMaterializationContext {
            workspace_id: "workspace-1".to_string(),
            runtime_id: "runtime-1".to_string(),
            operation_id: "operation-1".to_string(),
            config_revision: 1,
            config_projection_digest: "sha256:projection".to_string(),
            cache_generation: 0,
            ssh,
        };

        let mut https = request(repo.path());
        https.repository.source = workspace_api::RepositorySource {
            kind: workspace_api::RepositorySourceKind::Https,
            uri: "https://token@example.test/repo.git".to_string(),
        };
        https.materialization = Some(context(None));
        assert_eq!(
            materializer
                .materialize(&worker_ref(1), &https)
                .unwrap_err()
                .code,
            "working_directory_repository_source_invalid"
        );

        let mut http = request(repo.path());
        http.repository.source = workspace_api::RepositorySource {
            kind: workspace_api::RepositorySourceKind::Http,
            uri: "http://example.test/repo.git".to_string(),
        };
        http.materialization = Some(context(None));
        RuntimeGitCacheMaterializer::validate_request(&http).unwrap();
        assert_eq!(
            repository_transport_warning(http.repository.source.kind),
            Some("plain_http_transport")
        );

        let mut ssh = request(repo.path());
        ssh.repository.source = workspace_api::RepositorySource {
            kind: workspace_api::RepositorySourceKind::Ssh,
            uri: "ssh://git@example.test/repo.git".to_string(),
        };
        ssh.materialization = Some(context(Some(
            crate::catalog::RepositorySshMaterializationAccess {
                credential_id: "credential-1".to_string(),
                credential_revision: 1,
                host_trust_id: "trust-1".to_string(),
                host_trust_revision: 1,
                access: workspace_api::RepositoryAccessMode::ReadOnly,
                expires_at_epoch_seconds: u64::MAX,
                private_key: crate::catalog::SensitiveString::new("PRIVATE KEY placeholder"),
                known_hosts_entry: crate::catalog::SensitiveString::new(
                    "other.test ssh-ed25519 placeholder",
                ),
            },
        )));
        assert_eq!(
            materializer
                .materialize(&worker_ref(2), &ssh)
                .unwrap_err()
                .code,
            "working_directory_remote_repository_host_trust_mismatch"
        );
    }

    #[test]
    fn unsupported_remote_and_non_git_provider_return_typed_diagnostics() {
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
        let mut remote = request(Path::new("."));
        remote.repository.source = workspace_api::RepositorySource {
            kind: workspace_api::RepositorySourceKind::Ssh,
            uri: "ssh://git@example.invalid/repo.git".to_string(),
        };
        remote.materialization = Some(crate::catalog::RepositoryMaterializationContext {
            workspace_id: "workspace-1".to_string(),
            runtime_id: "runtime-1".to_string(),
            operation_id: "operation-1".to_string(),
            config_revision: 1,
            config_projection_digest: "sha256:projection".to_string(),
            cache_generation: 0,
            ssh: None,
        });
        let error = materializer
            .materialize(&worker_ref(1), &remote)
            .unwrap_err();
        assert_eq!(
            error.code,
            "working_directory_remote_repository_access_required"
        );

        let mut non_git = remote;
        non_git.repository.provider = "archive".to_string();
        non_git.repository.source.uri = ".".to_string();
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
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
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
        assert_eq!(listed[0].summary.creation_selector.as_deref(), Some("HEAD"));
        assert_eq!(listed[0].summary.current_selector, None);
        assert_eq!(
            listed[0].summary.current_ref,
            listed[0].summary.creation_ref
        );
    }

    #[test]
    fn working_directory_observes_current_selector_and_ref_without_changing_creation_evidence() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
        let working_directory = materializer.create(&request(repo.path())).unwrap();
        let bound = materializer
            .bind_working_directory(&working_directory.working_directory.id, None)
            .unwrap();
        let initial_ref = bound.status().summary.creation_ref.expect("creation ref");

        git(&bound.root, &["switch", "-c", "observed-branch"]);
        fs::write(bound.root.join("observed.txt"), "observed\n").unwrap();
        git(&bound.root, &["add", "observed.txt"]);
        git(&bound.root, &["commit", "-m", "advance workdir"]);

        let summary = materializer.list_working_directories().unwrap()[0]
            .summary
            .clone();
        assert_eq!(summary.creation_selector.as_deref(), Some("HEAD"));
        assert_eq!(summary.creation_ref.as_deref(), Some(initial_ref.as_str()));
        assert_eq!(summary.current_selector.as_deref(), Some("observed-branch"));
        assert_ne!(summary.current_ref.as_deref(), Some(initial_ref.as_str()));
    }

    #[test]
    fn relative_cwd_rejects_absolute_parent_nonexistent_file_and_symlink_escape() {
        let repo = create_clean_repo();
        fs::create_dir_all(repo.path().join("inside")).unwrap();
        fs::write(repo.path().join("inside/file.txt"), "file\n").unwrap();
        git(repo.path(), &["add", "inside/file.txt"]);
        git(repo.path(), &["commit", "-m", "add inside"]);
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
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
    fn cleanup_working_directory_removes_worktree_and_record() {
        let repo = create_clean_repo();
        let runtime_root = tempfile::tempdir().unwrap();
        let materializer = RuntimeGitCacheMaterializer::new(runtime_root.path());
        let binding = materializer
            .materialize(&worker_ref(1), &request(repo.path()))
            .unwrap();
        let root = binding.root.clone();
        let record_root = binding.working_directory_root().to_path_buf();

        let status = materializer
            .cleanup_working_directory(&binding.working_directory.id)
            .unwrap();

        assert_eq!(status.summary.status, WorkingDirectoryStatusKind::NotFound);
        assert!(!root.exists());
        assert!(!record_root.exists());
    }
}
