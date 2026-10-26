#[path = "worker_environment/response_loss.rs"]
mod response_loss;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    FixtureAllocation, HarnessError, Result, allocate_fixture, command_display, now_ms,
    product_target_dir, test_profile, workspace_root,
};

const STARTUP_WAIT: Duration = Duration::from_secs(15);
const EXIT_WAIT: Duration = Duration::from_secs(5);
const SETUP_WAIT: Duration = Duration::from_secs(15);
const HTTP_WAIT_STEP: Duration = Duration::from_millis(50);
const MAX_PROCESS_LOG_BYTES: usize = 256 * 1024;
const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;
static WORKER_ENV_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerBinaryRecord {
    pub name: String,
    pub path: PathBuf,
    pub sha256: String,
    pub provider: String,
    pub build_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerBinaryProviderInfo {
    pub workspace_root: PathBuf,
    pub profile: String,
    pub binaries: Vec<WorkerBinaryRecord>,
}

impl WorkerBinaryProviderInfo {
    pub fn binary(&self, name: &str) -> Result<PathBuf> {
        self.binaries
            .iter()
            .find(|record| record.name == name)
            .map(|record| record.path.clone())
            .ok_or_else(|| HarnessError::Protocol(format!("binary provider omitted {name}")))
    }
}

pub fn worker_binary_provider_info() -> Result<WorkerBinaryProviderInfo> {
    static INFO: OnceLock<std::result::Result<WorkerBinaryProviderInfo, String>> = OnceLock::new();
    match INFO.get_or_init(|| resolve_worker_binaries().map_err(|error| error.to_string())) {
        Ok(info) => Ok(info.clone()),
        Err(message) => Err(HarnessError::Protocol(message.clone())),
    }
}

fn resolve_worker_binaries() -> Result<WorkerBinaryProviderInfo> {
    let workspace_root = workspace_root()?;
    let profile = test_profile();
    let mut records = Vec::new();
    let yoi_override = resolve_override("YOI_E2E_BIN", &workspace_root)?;
    let server_override = resolve_override("YOI_E2E_SERVER_BIN", &workspace_root)?;
    let runtime_override = resolve_override("YOI_E2E_RUNTIME_BIN", &workspace_root)?;
    let mut build_command = None;
    if yoi_override.is_none() || server_override.is_none() || runtime_override.is_none() {
        let cargo = PathBuf::from(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        let mut args = vec![
            "build".to_string(),
            "-p".to_string(),
            "yoi".to_string(),
            "--bin".to_string(),
            "yoi".to_string(),
            "-p".to_string(),
            "yoi-workspace-server".to_string(),
            "--bin".to_string(),
            "yoi-server".to_string(),
            "-p".to_string(),
            "worker-runtime".to_string(),
            "--bin".to_string(),
            "yoi-runtime".to_string(),
            "--target-dir".to_string(),
            product_target_dir()?.to_string_lossy().into_owned(),
        ];
        if profile == "release" {
            args.push("--release".to_string());
        }
        let rendered = command_display(&cargo, &args);
        eprintln!("yoi-e2e worker binary provider=cargo-build command={rendered}");
        let output = Command::new(&cargo)
            .args(&args)
            .current_dir(&workspace_root)
            .output()?;
        if !output.status.success() {
            return Err(HarnessError::CommandFailed {
                program: cargo,
                args,
                status: output.status,
                stdout: bounded_text(&output.stdout, MAX_COMMAND_OUTPUT_BYTES),
                stderr: bounded_text(&output.stderr, MAX_COMMAND_OUTPUT_BYTES),
            });
        }
        build_command = Some(rendered);
    }

    let profile_dir = product_target_dir()?.join(&profile);
    let (yoi_path, yoi_provider, yoi_build) = match yoi_override {
        Some(path) => (path, "YOI_E2E_BIN".to_string(), None),
        None => (
            profile_dir.join(executable_name("yoi")),
            "cargo-build".to_string(),
            build_command.clone(),
        ),
    };
    records.push(binary_record("yoi", yoi_path, yoi_provider, yoi_build)?);
    let (server_path, server_provider, server_build) = match server_override {
        Some(path) => (path, "YOI_E2E_SERVER_BIN".to_string(), None),
        None => (
            profile_dir.join(executable_name("yoi-server")),
            "cargo-build".to_string(),
            build_command.clone(),
        ),
    };
    let (runtime_path, runtime_provider, runtime_build) = match runtime_override {
        Some(path) => (path, "YOI_E2E_RUNTIME_BIN".to_string(), None),
        None => (
            profile_dir.join(executable_name("yoi-runtime")),
            "cargo-build".to_string(),
            build_command,
        ),
    };
    records.push(binary_record(
        "yoi-server",
        server_path,
        server_provider,
        server_build,
    )?);
    records.push(binary_record(
        "yoi-runtime",
        runtime_path,
        runtime_provider,
        runtime_build,
    )?);

    let info = WorkerBinaryProviderInfo {
        workspace_root: workspace_root.clone(),
        profile,
        binaries: records,
    };
    let artifact_dir = workspace_root.join("target/e2e-artifacts");
    fs::create_dir_all(&artifact_dir)?;
    fs::write(
        artifact_dir.join("worker-binary-provider.json"),
        serde_json::to_vec_pretty(&info)?,
    )?;
    Ok(info)
}

fn resolve_override(name: &str, workspace_root: &Path) -> Result<Option<PathBuf>> {
    let Some(path) = std::env::var_os(name) else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    };
    if !path.is_file() {
        return Err(HarnessError::MissingBinary(path));
    }
    Ok(Some(path))
}

fn binary_record(
    name: &str,
    path: PathBuf,
    provider: String,
    build_command: Option<String>,
) -> Result<WorkerBinaryRecord> {
    if !path.is_file() {
        return Err(HarnessError::MissingBinary(path));
    }
    let bytes = fs::read(&path)?;
    let digest = Sha256::digest(bytes);
    Ok(WorkerBinaryRecord {
        name: name.to_string(),
        path,
        sha256: format!(
            "sha256:{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ),
        provider,
        build_command,
    })
}

fn executable_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WorkerProcessShutdownOutcome {
    #[default]
    NotStarted,
    Succeeded,
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerEnvironmentCleanupReport {
    pub fixture_root: PathBuf,
    pub artifacts_dir: PathBuf,
    pub cleanup_attempted: bool,
    pub cleanup_success: bool,
    pub fixture_root_exists_after: bool,
    pub cleanup_error: Option<String>,
    pub runtime_shutdown: WorkerProcessShutdownOutcome,
    pub server_shutdown: WorkerProcessShutdownOutcome,
    pub root_cleanup_success: bool,
    pub root_cleanup_error: Option<String>,
    pub report_path: PathBuf,
}

/// 隔離したHOME／XDG領域と実Backend／Runtime subprocessを所有するfixture。
///
/// HTTP clientはproxyとredirectを使わず、テスト対象の接続先を明示する。
/// 終了時に所有processと一時領域を片付け、artifactとcleanup結果を残す。
#[derive(Debug)]
pub struct WorkerE2eEnvironment {
    temp_root: Option<tempfile::TempDir>,
    pub root: PathBuf,
    pub home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub xdg_state_home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_runtime_dir: PathBuf,
    pub server_data_dir: PathBuf,
    pub runtime_data_dir: PathBuf,
    pub server_config: PathBuf,
    pub artifacts_dir: PathBuf,
    pub workspace_id: Option<String>,
    pub runtime_id: String,
    pub account_handle: String,
    pub server_url: Option<String>,
    pub runtime_url: Option<String>,
    server: Option<ManagedProcess>,
    runtime: Option<ManagedProcess>,
    runtime_shutdown: WorkerProcessShutdownOutcome,
    server_shutdown: WorkerProcessShutdownOutcome,
    shutdown_error: Option<String>,
    runtime_public_identity: Option<Value>,
    access_token: Option<String>,
    client: Client,
    binaries: WorkerBinaryProviderInfo,
}

impl WorkerE2eEnvironment {
    pub fn new() -> Result<Self> {
        let allocation = allocate_fixture("worker")?;
        Self::from_allocation(allocation)
    }

    fn from_allocation(allocation: FixtureAllocation) -> Result<Self> {
        let unique = format!(
            "{}-{}-{}",
            std::process::id(),
            now_ms(),
            WORKER_ENV_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let server_data_dir = allocation.xdg_data_home.join("yoi/server");
        let runtime_data_dir = allocation.root.join("runtime-data");
        assert_absent_or_empty(&server_data_dir, "Server data directory")?;
        assert_absent_or_empty(&runtime_data_dir, "Runtime data directory")?;
        let server_config = allocation.xdg_config_home.join("yoi/server.toml");
        if let Some(parent) = server_config.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &server_config,
            "[browser]\npublic_url = \"http://localhost:5173\"\n",
        )?;
        let binaries = worker_binary_provider_info()?;
        fs::write(
            allocation.artifacts_dir.join("binary-provider.json"),
            serde_json::to_vec_pretty(&binaries)?,
        )?;
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|error| HarnessError::Protocol(format!("build E2E HTTP client: {error}")))?;
        let environment = Self {
            temp_root: Some(allocation.temp_root),
            root: allocation.root,
            home: allocation.home,
            xdg_data_home: allocation.xdg_data_home,
            xdg_state_home: allocation.xdg_state_home,
            xdg_config_home: allocation.xdg_config_home,
            xdg_runtime_dir: allocation.xdg_runtime_dir,
            server_data_dir,
            runtime_data_dir,
            server_config,
            artifacts_dir: allocation.artifacts_dir,
            workspace_id: None,
            runtime_id: format!("runtime-{unique}"),
            account_handle: format!("e2e-{}-{}", std::process::id(), unique.replace('-', "")),
            server_url: None,
            runtime_url: None,
            server: None,
            runtime: None,
            runtime_shutdown: WorkerProcessShutdownOutcome::NotStarted,
            server_shutdown: WorkerProcessShutdownOutcome::NotStarted,
            shutdown_error: None,
            runtime_public_identity: None,
            access_token: None,
            client,
            binaries,
        };
        environment.write_metadata("created")?;
        Ok(environment)
    }

    pub fn start(&mut self) -> Result<()> {
        self.start_backend()?;
        self.start_runtime()
    }

    /// 空のDBでBackendだけを起動し、Account／Workspaceは作成しない。
    pub fn start_empty_backend(&mut self) -> Result<()> {
        assert_absent_or_empty(&self.server_data_dir, "Server data directory")?;
        assert_absent_or_empty(&self.runtime_data_dir, "Runtime data directory")?;
        self.start_server_process()
    }

    /// 同じ隔離DB・設定でBackendを再起動する。Runtime付きfixtureには使わない。
    pub fn restart_backend(&mut self) -> Result<()> {
        if self.runtime.is_some() || self.server.is_none() {
            return Err(HarnessError::Protocol(
                "Backend-only restart requires a running Server and no Runtime".to_string(),
            ));
        }
        shutdown_process(&mut self.server, &mut self.server_shutdown);
        if let WorkerProcessShutdownOutcome::Failed { error } = &self.server_shutdown {
            return Err(HarnessError::Protocol(error.clone()));
        }
        self.server_url = None;
        self.start_server_process()
    }

    /// host CLIだけで管理者を明示設定する。初回bearerはartifactへ保存しない。
    pub fn setup_hosting_admin(&self, handle: &str, create_user: bool) -> Result<Value> {
        let binary = self.binaries.binary("yoi-server")?;
        let mut args = vec!["hosting-admin", "--handle", handle];
        if create_user {
            args.push("--create-user");
        }
        let output = self.run_command(&binary, &args, true)?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    /// 旧host bootstrap CLIは一般userを作る。認証情報はartifactへ保存しない。
    pub fn bootstrap_auth_user(&self, handle: &str) -> Result<Value> {
        let binary = self.binaries.binary("yoi-server")?;
        let output = self.run_command(&binary, &["bootstrap-auth", "--handle", handle], true)?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    fn start_server_process(&mut self) -> Result<()> {
        if self.server.is_some() {
            return Err(HarnessError::Protocol(
                "Server is already running".to_string(),
            ));
        }
        let server_binary = self.binaries.binary("yoi-server")?;
        let mut command = self.isolated_command(&server_binary);
        command.args([
            "serve",
            "--listen",
            "127.0.0.1:0",
            "--config",
            self.server_config.to_string_lossy().as_ref(),
        ]);
        let server = ManagedProcess::spawn(
            "server",
            command,
            &self.artifacts_dir.join("server.stdout.log"),
            &self.artifacts_dir.join("server.stderr.log"),
        )?;
        let server_addr = server.wait_for_address("on http://", STARTUP_WAIT)?;
        let server_url = format!("http://{server_addr}");
        self.server = Some(server);
        self.server_url = Some(server_url.clone());
        self.wait_for_server_health(&server_url)?;
        self.write_metadata("empty-backend-running")?;
        Ok(())
    }

    /// Start a store populated by the Backend's typed test-support API. The seed
    /// supplies fixed catalog IDs, but the bearer must be freshly issued for this
    /// environment. No database mutation or fixture credential import occurs here.
    pub fn start_seeded_backend(
        &mut self,
        workspace_id: String,
        runtime_id: String,
        access_token: String,
    ) -> Result<()> {
        if self.server.is_some() || self.runtime.is_some() || self.workspace_id.is_some() {
            return Err(HarnessError::Protocol(
                "fixture seed requires an unstarted environment".into(),
            ));
        }
        if workspace_id.is_empty() || runtime_id.is_empty() || access_token.is_empty() {
            return Err(HarnessError::Protocol(
                "fixture seed omitted an identity or fresh bearer".into(),
            ));
        }
        if !self.server_data_dir.join("server.db").is_file() {
            return Err(HarnessError::Protocol(
                "typed Backend seed has not created server.db".into(),
            ));
        }
        if self.runtime_public_identity.is_some() {
            if self.runtime_id != runtime_id {
                return Err(HarnessError::Protocol(
                    "seeded Runtime ID differs from initialized identity".into(),
                ));
            }
        } else {
            assert_absent_or_empty(&self.runtime_data_dir, "Runtime data directory")?;
        }
        self.workspace_id = Some(workspace_id);
        self.runtime_id = runtime_id;
        self.access_token = Some(access_token);
        self.start_server_process()?;
        self.export_workspace_public_identity()?;
        self.write_metadata("seeded-backend-running")
    }

    pub fn start_backend(&mut self) -> Result<()> {
        self.start_empty_backend()?;
        let server_binary = self.binaries.binary("yoi-server")?;
        let server_url = self.required_server_url()?.to_string();
        let bootstrap = self.run_command(
            &server_binary,
            &[
                "bootstrap-auth",
                "--handle",
                &self.account_handle,
                "--display-name",
                "Yoi Worker E2E",
                "--label",
                "worker-e2e-bootstrap",
            ],
            true,
        )?;
        let bootstrap: Value = serde_json::from_slice(&bootstrap.stdout)?;
        self.access_token = bootstrap
            .get("access_token")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if self.access_token.is_none() {
            return Err(HarnessError::Protocol(
                "bootstrap-auth omitted access_token".to_string(),
            ));
        }

        let created = self.request_json(
            self.client
                .post(format!("{server_url}/api/workspaces"))
                .json(&serde_json::json!({
                    "operation_key": format!("worker-e2e-{}", self.runtime_id),
                    "display_name": format!("Worker E2E {}", self.runtime_id),
                    "repository": {
                        "repository_key": "fixture",
                        "uri": "https://example.invalid/yoi-e2e.git",
                        "default_ref": "develop"
                    }
                })),
            "create Workspace",
        )?;
        let workspace_id = json_string(&created, "/workspace/workspace_id")?;
        self.workspace_id = Some(workspace_id);
        self.export_workspace_public_identity()?;
        self.write_metadata("backend-running")?;
        Ok(())
    }

    fn export_workspace_public_identity(&self) -> Result<()> {
        let server_url = self.required_server_url()?;
        let workspace_id = self.required_workspace_id()?;
        let identity = self.request_json(
            self.client.get(format!(
                "{server_url}/api/w/{workspace_id}/settings/signing-identity"
            )),
            "read Workspace signing identity",
        )?;
        let public_bundle = identity.get("public_bundle").cloned().ok_or_else(|| {
            HarnessError::Protocol("Workspace public bundle is absent".to_string())
        })?;
        let bundle_path = self.root.join("workspace-public-identity.json");
        fs::write(&bundle_path, serde_json::to_vec_pretty(&public_bundle)?)?;
        Ok(())
    }

    fn start_runtime(&mut self) -> Result<()> {
        self.start_runtime_with_seed(|_| Ok(()))?;
        self.assert_connected_and_empty()
    }

    /// Generate fresh keys before offline Backend registration when fixed IDs are
    /// required. Returns only the public bundle; no fixture keys are accepted.
    pub fn initialize_runtime_identity(&mut self, runtime_id: String) -> Result<Value> {
        if self.runtime_public_identity.is_some() || self.runtime.is_some() {
            return Err(HarnessError::Protocol(
                "Runtime identity is already initialized".into(),
            ));
        }
        assert_absent_or_empty(&self.runtime_data_dir, "Runtime data directory")?;
        self.runtime_id = runtime_id;
        let binary = self.binaries.binary("yoi-runtime")?;
        self.run_command(
            &binary,
            &[
                "identity",
                "init",
                "--runtime-id",
                &self.runtime_id,
                "--fs-root",
                self.runtime_data_dir.to_string_lossy().as_ref(),
            ],
            false,
        )?;
        let identity = self.run_command(
            &binary,
            &[
                "identity",
                "show",
                "--json",
                "--fs-root",
                self.runtime_data_dir.to_string_lossy().as_ref(),
            ],
            false,
        )?;
        let identity: Value = serde_json::from_slice(&identity.stdout)?;
        self.runtime_public_identity = Some(identity.clone());
        Ok(identity)
    }

    /// Initialize fresh Runtime keys/trust, then invoke the typed Runtime seed
    /// before starting its process. The actual Backend URL is now available.
    pub fn start_runtime_with_seed(
        &mut self,
        seed: impl FnOnce(&Self) -> Result<()>,
    ) -> Result<()> {
        if self.runtime.is_some() {
            return Err(HarnessError::Protocol("Runtime is already running".into()));
        }
        if self.runtime_public_identity.is_none() {
            self.initialize_runtime_identity(self.runtime_id.clone())?;
        }
        let bundle_path = self.root.join("workspace-public-identity.json");
        let workspace_id = self.required_workspace_id()?.to_string();
        let runtime_binary = self.binaries.binary("yoi-runtime")?;
        self.run_command(
            &runtime_binary,
            &[
                "trust-workspace",
                "add",
                "--bundle",
                bundle_path.to_string_lossy().as_ref(),
                "--fs-root",
                self.runtime_data_dir.to_string_lossy().as_ref(),
            ],
            false,
        )?;

        let enrollment = self.run_command(
            &runtime_binary,
            &[
                "trust-workspace",
                "show",
                "--workspace-id",
                &workspace_id,
                "--fs-root",
                self.runtime_data_dir.to_string_lossy().as_ref(),
            ],
            false,
        )?;
        let enrollment: Value = serde_json::from_slice(&enrollment.stdout)?;
        if json_string(&enrollment, "/trust_id")?.is_empty()
            || enrollment.get("workspace_id").and_then(Value::as_str) != Some(workspace_id.as_str())
            || enrollment.get("state").and_then(Value::as_str) != Some("active")
        {
            return Err(HarnessError::Protocol(
                "Runtime did not retain active trust for the fixture Workspace".to_string(),
            ));
        }

        seed(self)?;
        self.write_metadata("runtime-seeded")?;
        self.start_runtime_process()
    }

    fn start_runtime_process(&mut self) -> Result<()> {
        if self.runtime.is_some() {
            return Err(HarnessError::Protocol("Runtime is already running".into()));
        }
        let runtime_binary = self.binaries.binary("yoi-runtime")?;
        let runtime_identity = self
            .runtime_public_identity
            .clone()
            .ok_or_else(|| HarnessError::Protocol("Runtime identity is not initialized".into()))?;
        let server_url = self.required_server_url()?.to_string();
        let workspace_id = self.required_workspace_id()?.to_string();
        let mut command = self.isolated_command(&runtime_binary);
        command.args([
            "--bind",
            "127.0.0.1:0",
            "--display-name",
            &format!("E2E {}", self.runtime_id),
            "--store",
            "fs",
            "--fs-root",
            self.runtime_data_dir.to_string_lossy().as_ref(),
        ]);
        let runtime = ManagedProcess::spawn(
            "runtime",
            command,
            &self.artifacts_dir.join("runtime.stdout.log"),
            &self.artifacts_dir.join("runtime.stderr.log"),
        )?;
        let runtime_addr = runtime.wait_for_address("listening on ", STARTUP_WAIT)?;
        let runtime_url = format!("http://{runtime_addr}");
        self.runtime = Some(runtime);
        self.runtime_url = Some(runtime_url.clone());

        let registered = self.request_json(
            self.client
                .get(format!("{server_url}/api/w/{workspace_id}/runtimes")),
            "observe Runtime registration before endpoint update",
        )?;
        let expected_binding_id = registered
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("runtime_id").and_then(Value::as_str) == Some(self.runtime_id.as_str())
                })
            })
            .and_then(|item| item.pointer("/management/binding/binding_id"))
            .cloned();
        self.request_json(
            self.client
                .post(format!("{server_url}/api/w/{workspace_id}/runtimes"))
                .json(&serde_json::json!({
                    "public_bundle": runtime_identity,
                    "display_name": format!("E2E {}", self.runtime_id),
                    "endpoint": runtime_url,
                    "expected_binding_id": expected_binding_id,
                })),
            "register Runtime",
        )?;
        self.assert_connected()?;
        self.write_metadata("running")?;
        Ok(())
    }

    /// Kill/reap only the Runtime child owned by this environment, retaining roots
    /// and identities. Restart neither seeds storage nor rotates restore requests.
    pub fn crash_and_restart_runtime(&mut self) -> Result<()> {
        let runtime = self
            .runtime
            .as_mut()
            .ok_or_else(|| HarnessError::Protocol("Runtime is not running".into()))?;
        let pid = runtime.id();
        let status = runtime.kill_and_reap(EXIT_WAIT)?;
        runtime.join_readers_until(Instant::now() + EXIT_WAIT)?;
        fs::write(
            self.artifacts_dir.join("runtime-crash.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "owned_pid": pid, "reaped": true, "exit_status": status.to_string(),
                "same_runtime_id": self.runtime_id,
            }))?,
        )?;
        self.runtime.take();
        for stream in ["stdout", "stderr"] {
            fs::rename(
                self.artifacts_dir.join(format!("runtime.{stream}.log")),
                self.artifacts_dir
                    .join(format!("runtime-before-crash.{stream}.log")),
            )?;
        }
        self.runtime_url = None;
        self.runtime_shutdown = WorkerProcessShutdownOutcome::NotStarted;
        self.start_runtime_process()
    }

    pub fn assert_connected(&self) -> Result<()> {
        let server_url = self.required_server_url()?;
        let workspace_id = self.required_workspace_id()?;
        // connection testは実署名要求。保存済みの接続flagを認証成功の代わりにしない。
        let connection = self.request_json(
            self.client.post(format!(
                "{server_url}/api/w/{workspace_id}/runtimes/{}/connection-tests",
                self.runtime_id
            )),
            "verify Runtime connection",
        )?;
        require_compatible_connection(&connection)?;
        let binding_id = json_string(&connection, "/binding_id")?;
        let runtimes = self.request_json(
            self.client
                .get(format!("{server_url}/api/w/{workspace_id}/runtimes")),
            "list Runtimes",
        )?;
        require_registered_runtime(&runtimes, &self.runtime_id, &binding_id)
    }

    pub fn list_workers(&self) -> Result<Value> {
        let server_url = self.required_server_url()?;
        let workspace_id = self.required_workspace_id()?;
        self.request_json(
            self.client
                .get(format!("{server_url}/api/w/{workspace_id}/workers")),
            "list Workers",
        )
    }

    pub fn assert_connected_and_empty(&self) -> Result<()> {
        self.assert_connected()?;
        let workers = self.list_workers()?;
        require_empty_worker_list(&workers)
    }

    fn worker_url(&self, worker_id: &str, suffix: &str) -> Result<String> {
        let mut url = reqwest::Url::parse(self.required_server_url()?)
            .map_err(|error| HarnessError::Protocol(format!("invalid Server URL: {error}")))?;
        url.path_segments_mut()
            .map_err(|_| HarnessError::Protocol("Server URL has no path segments".into()))?
            .extend([
                "api",
                "w",
                self.required_workspace_id()?,
                "runtimes",
                &self.runtime_id,
                "workers",
                worker_id,
            ]);
        if !suffix.is_empty() {
            url.path_segments_mut().unwrap().extend(suffix.split('/'));
        }
        Ok(url.into())
    }

    pub fn worker_detail(&self, worker_id: &str) -> Result<Value> {
        self.request_json(
            self.client.get(self.worker_url(worker_id, "")?),
            "read Worker detail",
        )
    }

    /// Exactly one restore submission. No retries, replacement token or new intent.
    pub fn restore_worker(
        &self,
        worker_id: &str,
        request_id: &str,
        expected_observation_token: &str,
    ) -> Result<Value> {
        self.request_json(
            self.client
                .post(self.worker_url(worker_id, "restore")?)
                .timeout(STARTUP_WAIT)
                .json(&serde_json::json!({
                    "request_id": request_id,
                    "expected_observation_token": expected_observation_token,
                })),
            "restore Worker",
        )
    }

    /// Forward one unchanged public restore request through an owned loopback
    /// relay, consume its complete upstream response, and close without delivering
    /// any response bytes. The returned JSON is relay evidence, not caller success.
    pub fn restore_worker_losing_response(
        &self,
        worker_id: &str,
        request_id: &str,
        expected_observation_token: &str,
    ) -> Result<Value> {
        let token = self.access_token.as_deref().ok_or_else(|| {
            HarnessError::Protocol("Backend access token is not initialized".into())
        })?;
        let request = self
            .client
            .post(self.worker_url(worker_id, "restore")?)
            .bearer_auth(token)
            .header(reqwest::header::CONNECTION, "close")
            .timeout(STARTUP_WAIT + EXIT_WAIT)
            .json(&serde_json::json!({
                "request_id": request_id,
                "expected_observation_token": expected_observation_token,
            }))
            .build()
            .map_err(|_| HarnessError::Protocol("build response-loss request".into()))?;
        let evidence = response_loss::drop_response(&self.client, request)?;
        self.append_operation(
            "restore response consumed by relay, caller transport failed",
            0,
        )?;
        Ok(evidence)
    }

    pub fn stop_worker(&self, worker_id: &str) -> Result<Value> {
        self.request_json(
            self.client
                .post(self.worker_url(worker_id, "stop")?)
                .json(&serde_json::json!({})),
            "stop Worker",
        )
    }

    /// Authorization stays in memory; callers must not persist or debug this request.
    pub fn worker_protocol_request(&self, worker_id: &str) -> Result<reqwest::blocking::Request> {
        let token = self.access_token.as_deref().ok_or_else(|| {
            HarnessError::Protocol("Backend access token is not initialized".into())
        })?;
        self.client
            .get(self.worker_url(worker_id, "protocol/ws")?)
            .bearer_auth(token)
            .build()
            .map_err(|_| HarnessError::Protocol("build Worker protocol request".into()))
    }

    /// The other environment must reject this environment's fresh bearer even
    /// when both contain identical Workspace/Runtime/Worker IDs. Never logs tokens.
    pub fn assert_authentication_isolated_from(&self, other: &Self) -> Result<()> {
        let token = self
            .access_token
            .as_deref()
            .ok_or_else(|| HarnessError::Protocol("missing source bearer".into()))?;
        if other.access_token.as_deref() == Some(token) {
            return Err(HarnessError::Protocol(
                "environments share authentication material".into(),
            ));
        }
        let (server, workspace) = other.backend_target()?;
        let response = self
            .client
            .get(format!("{server}/api/w/{workspace}/workers"))
            .bearer_auth(token)
            .send()
            .map_err(|_| {
                HarnessError::Protocol("cross-environment authorization request failed".into())
            })?;
        let status = response.status().as_u16();
        other.append_operation("reject other environment bearer", status)?;
        if status != 401 {
            return Err(HarnessError::Protocol(format!(
                "cross-environment bearer was not rejected with 401: {status}"
            )));
        }
        Ok(())
    }

    pub fn install_backend_token(&self, config_home: &Path) -> Result<PathBuf> {
        let server_url = self.required_server_url()?;
        let access_token = self.access_token.as_deref().ok_or_else(|| {
            HarnessError::Protocol("Backend access token is not initialized".to_string())
        })?;
        let token_path = config_home.join("yoi/backend-tokens.json");
        if let Some(parent) = token_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &token_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "tokens": {
                    server_url: {
                        "token_type": "Bearer",
                        "access_token": access_token,
                    }
                }
            }))?,
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(token_path)
    }

    pub fn create_planning_ticket(&self, title: &str) -> Result<String> {
        let server_url = self.required_server_url()?;
        let workspace_id = self.required_workspace_id()?;
        let created = self.request_json(
            self.client
                .post(format!("{server_url}/api/w/{workspace_id}/tickets"))
                .json(&serde_json::json!({
                    "title": title,
                    "slug": null,
                    "kind": "task",
                    "priority": "P2",
                    "labels": [],
                    "body": "Backend-owned panel E2E fixture.",
                    "author": null,
                    "assignee": null,
                    "readiness": null,
                    "risk_flags": [],
                    "workflow_state": "open",
                    "queued_by": null,
                    "queued_at": null,
                    "repository_key": null,
                    "ref_selector": null,
                })),
            "create Ticket",
        )?;
        json_string(&created, "/id")
    }

    pub fn capture_panel_seed(&self) -> Result<()> {
        let server_url = self.required_server_url()?;
        let workspace_id = self.required_workspace_id()?;
        let tickets = self.request_json(
            self.client.get(format!(
                "{server_url}/api/w/{workspace_id}/tickets/search?state=active"
            )),
            "list panel fixture Tickets",
        )?;
        let objectives = self.request_json(
            self.client.get(format!(
                "{server_url}/api/w/{workspace_id}/objectives?limit=1000"
            )),
            "list panel fixture Objectives",
        )?;
        fs::write(
            self.artifacts_dir.join("panel-seed.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "tickets": tickets,
                "objectives": objectives,
            }))?,
        )?;
        Ok(())
    }

    pub fn backend_target(&self) -> Result<(&str, &str)> {
        Ok((self.required_server_url()?, self.required_workspace_id()?))
    }

    pub fn server_port(&self) -> Result<u16> {
        url_port(self.required_server_url()?)
    }

    pub fn runtime_port(&self) -> Result<u16> {
        url_port(
            self.runtime_url
                .as_deref()
                .ok_or_else(|| HarnessError::Protocol("Runtime is not started".to_string()))?,
        )
    }

    pub fn server_pid(&self) -> Option<u32> {
        self.server.as_ref().map(ManagedProcess::id)
    }

    pub fn runtime_pid(&self) -> Option<u32> {
        self.runtime.as_ref().map(ManagedProcess::id)
    }

    pub fn shutdown(&mut self) -> Result<()> {
        // 一方の異常でも他方を回収し、再呼出しやroot削除で失敗を消さない。
        shutdown_process(&mut self.runtime, &mut self.runtime_shutdown);
        shutdown_process(&mut self.server, &mut self.server_shutdown);
        let mut errors = Vec::new();
        for (name, outcome) in [
            ("Runtime", &self.runtime_shutdown),
            ("Server", &self.server_shutdown),
        ] {
            if let WorkerProcessShutdownOutcome::Failed { error } = outcome {
                errors.push(format!("{name}: {error}"));
            }
        }
        if self.write_metadata("stopped").is_err() {
            errors.push("write stopped metadata failed".to_string());
        }
        if self.shutdown_error.is_none() && !errors.is_empty() {
            self.shutdown_error = Some(errors.join("; "));
        }
        match &self.shutdown_error {
            Some(error) => Err(HarnessError::Protocol(error.clone())),
            None => Ok(()),
        }
    }

    pub fn cleanup(mut self) -> Result<WorkerEnvironmentCleanupReport> {
        self.cleanup_inner(true)
    }

    fn cleanup_inner(&mut self, strict: bool) -> Result<WorkerEnvironmentCleanupReport> {
        let _ = self.shutdown();
        let mut root_cleanup_error = None;
        if let Some(temp_root) = self.temp_root.take()
            && temp_root.close().is_err()
        {
            root_cleanup_error = Some("remove fixture root failed".to_string());
        }
        let fixture_root_exists_after = self.root.exists();
        if fixture_root_exists_after && root_cleanup_error.is_none() {
            root_cleanup_error = Some("fixture root still exists".to_string());
        }
        let root_cleanup_success = root_cleanup_error.is_none() && !fixture_root_exists_after;
        let cleanup_error = match (&self.shutdown_error, &root_cleanup_error) {
            (Some(shutdown), Some(root)) => Some(format!("{shutdown}; {root}")),
            (Some(error), _) | (_, Some(error)) => Some(error.clone()),
            (None, None) => None,
        };
        let cleanup_success = self.shutdown_error.is_none() && root_cleanup_success;
        let report = WorkerEnvironmentCleanupReport {
            fixture_root: self.root.clone(),
            artifacts_dir: self.artifacts_dir.clone(),
            cleanup_attempted: true,
            cleanup_success,
            fixture_root_exists_after,
            cleanup_error,
            runtime_shutdown: self.runtime_shutdown.clone(),
            server_shutdown: self.server_shutdown.clone(),
            root_cleanup_success,
            root_cleanup_error,
            report_path: self.artifacts_dir.join("cleanup.json"),
        };
        fs::write(&report.report_path, serde_json::to_vec_pretty(&report)?)?;
        if strict && !report.cleanup_success {
            return Err(HarnessError::Protocol(format!(
                "Worker E2E cleanup failed; see {}",
                report.report_path.display()
            )));
        }
        Ok(report)
    }

    fn wait_for_server_health(&self, server_url: &str) -> Result<()> {
        let deadline = Instant::now() + STARTUP_WAIT;
        loop {
            let last = match self.client.get(format!("{server_url}/health")).send() {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => format!("HTTP {}", response.status()),
                Err(error) => error.to_string(),
            };
            if Instant::now() >= deadline {
                return Err(HarnessError::Protocol(format!(
                    "Server health did not become ready: {last}"
                )));
            }
            if let Some(server) = self.server.as_ref()
                && !server.is_running()
            {
                return Err(HarnessError::Protocol(
                    "Server exited before health became ready".to_string(),
                ));
            }
            thread::sleep(HTTP_WAIT_STEP);
        }
    }

    fn request_json(&self, request: RequestBuilder, operation: &str) -> Result<Value> {
        let request = if let Some(token) = self.access_token.as_deref() {
            request.bearer_auth(token)
        } else {
            request
        };
        let response = request
            .send()
            .map_err(|error| HarnessError::Protocol(format!("{operation}: {error}")))?;
        self.append_operation(operation, response.status().as_u16())?;
        decode_api_response(response, operation)
    }

    fn append_operation(&self, operation: &str, status: u16) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.artifacts_dir.join("operations.jsonl"))?;
        serde_json::to_writer(
            &mut file,
            &serde_json::json!({
                "ts_ms": now_ms(),
                "operation": operation,
                "status": status,
            }),
        )?;
        writeln!(file)?;
        Ok(())
    }

    fn run_command(
        &self,
        binary: &Path,
        args: &[&str],
        redact_stdout: bool,
    ) -> Result<std::process::Output> {
        self.run_command_with_timeout(binary, args, redact_stdout, SETUP_WAIT)
    }

    fn run_command_with_timeout(
        &self,
        binary: &Path,
        args: &[&str],
        redact_stdout: bool,
        timeout: Duration,
    ) -> Result<std::process::Output> {
        let args_owned = args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>();
        let mut command = self.isolated_command(binary);
        command.args(args).stdin(Stdio::null());
        // credentialを含むstdoutは呼出元へだけ返す。raw fileは作らない。
        let mut process = ManagedProcess::spawn_captured(
            "setup command",
            command,
            None,
            None,
            MAX_COMMAND_OUTPUT_BYTES + 1,
        )?;
        let result = process.collect_output(timeout);
        let output = result.as_ref().ok();
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.artifacts_dir.join("setup-commands.jsonl"))?;
        serde_json::to_writer(
            &mut log,
            &serde_json::json!({
                "ts_ms": now_ms(),
                "binary": binary,
                "args": args,
                "status": output.and_then(|output| output.status.code()),
                "stdout": if redact_stdout {
                    "<redacted>".to_string()
                } else {
                    output.map(|output| bounded_text(&output.stdout, MAX_COMMAND_OUTPUT_BYTES))
                        .unwrap_or_default()
                },
                "stderr": output.map(|output| bounded_text(&output.stderr, MAX_COMMAND_OUTPUT_BYTES))
                    .unwrap_or_default(),
                "error": result.as_ref().err().map(ToString::to_string),
                "env_clear": true,
            }),
        )?;
        writeln!(log)?;
        let output = result?;
        if !output.status.success() {
            return Err(HarnessError::CommandFailed {
                program: binary.to_path_buf(),
                args: args_owned,
                status: output.status,
                stdout: if redact_stdout {
                    "<redacted>".to_string()
                } else {
                    bounded_text(&output.stdout, MAX_COMMAND_OUTPUT_BYTES)
                },
                stderr: bounded_text(&output.stderr, MAX_COMMAND_OUTPUT_BYTES),
            });
        }
        Ok(output)
    }

    fn isolated_command(&self, binary: &Path) -> Command {
        let mut command = Command::new(binary);
        command
            .current_dir(&self.root)
            .env_clear()
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", &self.xdg_data_home)
            .env("XDG_STATE_HOME", &self.xdg_state_home)
            .env("XDG_CONFIG_HOME", &self.xdg_config_home)
            .env("XDG_RUNTIME_DIR", &self.xdg_runtime_dir);
        command
    }

    fn required_server_url(&self) -> Result<&str> {
        self.server_url
            .as_deref()
            .ok_or_else(|| HarnessError::Protocol("Server is not started".to_string()))
    }

    fn required_workspace_id(&self) -> Result<&str> {
        self.workspace_id
            .as_deref()
            .ok_or_else(|| HarnessError::Protocol("Workspace is not created".to_string()))
    }

    fn write_metadata(&self, phase: &str) -> Result<()> {
        fs::write(
            self.artifacts_dir.join("worker-environment.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "phase": phase,
                "fixture_root": self.root,
                "server_data_dir": self.server_data_dir,
                "runtime_data_dir": self.runtime_data_dir,
                "home": self.home,
                "xdg_data_home": self.xdg_data_home,
                "xdg_state_home": self.xdg_state_home,
                "xdg_config_home": self.xdg_config_home,
                "xdg_runtime_dir": self.xdg_runtime_dir,
                "server_config": self.server_config,
                "artifacts_dir": self.artifacts_dir,
                "workspace_id": self.workspace_id,
                "runtime_id": self.runtime_id,
                "account_handle": self.account_handle,
                "server_url": self.server_url,
                "runtime_url": self.runtime_url,
                "server_pid": self.server_pid(),
                "runtime_pid": self.runtime_pid(),
                "env_clear": true,
                "credentials_persisted_to_artifacts": false,
            }))?,
        )?;
        Ok(())
    }
}

// The operation is a caller-owned static label, never a response field. Refused
// response bodies may contain secrets; do not even collect them for diagnostics.
fn decode_api_response(response: reqwest::blocking::Response, operation: &str) -> Result<Value> {
    let status = response.status();
    if !status.is_success() {
        return Err(HarnessError::Protocol(format!(
            "{operation} returned HTTP {}",
            status.as_u16()
        )));
    }
    let mut body = std::io::Read::take(response, MAX_COMMAND_OUTPUT_BYTES as u64 + 1);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut body, &mut bytes)
        .map_err(|_| HarnessError::Protocol(format!("{operation}: response read failed")))?;
    if bytes.len() > MAX_COMMAND_OUTPUT_BYTES {
        return Err(HarnessError::Protocol(format!(
            "{operation}: response byte budget exceeded"
        )));
    }
    serde_json::from_slice(&bytes).map_err(HarnessError::from)
}

impl Drop for WorkerE2eEnvironment {
    fn drop(&mut self) {
        if self.temp_root.is_some() {
            let _ = self.cleanup_inner(false);
        } else if self.runtime.is_some() || self.server.is_some() {
            let _ = self.shutdown();
        }
    }
}

fn shutdown_process(
    process: &mut Option<ManagedProcess>,
    outcome: &mut WorkerProcessShutdownOutcome,
) {
    if let Some(mut process) = process.take() {
        let next = match process.shutdown(EXIT_WAIT) {
            Ok(_) => WorkerProcessShutdownOutcome::Succeeded,
            // shutdownはstatus／OSエラーだけを返し、captureしたstdout/stderrは含めない。
            Err(error) => WorkerProcessShutdownOutcome::Failed {
                error: error.to_string(),
            },
        };
        if !matches!(outcome, WorkerProcessShutdownOutcome::Failed { .. }) {
            *outcome = next;
        }
    }
}

#[derive(Debug)]
struct ManagedProcess {
    name: String,
    child: Child,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    readers: Vec<JoinHandle<std::io::Result<()>>>,
}

impl ManagedProcess {
    fn spawn(name: &str, command: Command, stdout_path: &Path, stderr_path: &Path) -> Result<Self> {
        Self::spawn_captured(
            name,
            command,
            Some(stdout_path.to_path_buf()),
            Some(stderr_path.to_path_buf()),
            MAX_PROCESS_LOG_BYTES,
        )
    }

    fn spawn_captured(
        name: &str,
        mut command: Command,
        stdout_path: Option<PathBuf>,
        stderr_path: Option<PathBuf>,
        capture_limit: usize,
    ) -> Result<Self> {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::Protocol(format!("{name} stdout was not piped")))?;
        let stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| HarnessError::Protocol(format!("{name} stderr was not piped")))?;
        let stdout = Arc::new(Mutex::new(Vec::new()));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let readers = vec![
            capture_stream(stdout_pipe, stdout_path, Arc::clone(&stdout), capture_limit),
            capture_stream(stderr_pipe, stderr_path, Arc::clone(&stderr), capture_limit),
        ];
        Ok(Self {
            name: name.to_string(),
            child,
            stdout,
            stderr,
            readers,
        })
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn is_running(&self) -> bool {
        unsafe { libc::kill(self.child.id() as i32, 0) == 0 }
    }

    fn wait_for_address(&self, marker: &str, timeout: Duration) -> Result<String> {
        let start = Instant::now();
        loop {
            // stdout/stderrの結合で補った改行をready記録と誤認しない。
            // readerが受信した各streamの完全な行だけを判定する。
            let address = [&self.stdout, &self.stderr]
                .into_iter()
                .find_map(|capture| {
                    let bytes = capture.lock().ok()?;
                    parse_address_after(&String::from_utf8_lossy(&bytes), marker)
                });
            if let Some(address) = address {
                return Ok(address);
            }
            let output = self.output();
            if start.elapsed() >= timeout {
                return Err(HarnessError::Protocol(format!(
                    "timed out waiting for {} address after {marker:?}; bounded output: {}",
                    self.name,
                    bounded_text(output.as_bytes(), 8 * 1024)
                )));
            }
            let rc = unsafe { libc::kill(self.child.id() as i32, 0) };
            if rc != 0 {
                return Err(HarnessError::Protocol(format!(
                    "{} exited before publishing its address; bounded output: {}",
                    self.name,
                    bounded_text(output.as_bytes(), 8 * 1024)
                )));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn output(&self) -> String {
        let stdout = self
            .stdout
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        let stderr = self
            .stderr
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        )
    }

    fn collect_output(&mut self, timeout: Duration) -> Result<std::process::Output> {
        let deadline = Instant::now() + timeout;
        let result = self.wait_until(deadline);
        let status = match result {
            Ok(Some(status)) => status,
            other => {
                // timeoutでも所有childをkillしてreapする。出力は診断へコピーしない。
                let _ = self.kill_and_reap(EXIT_WAIT);
                let _ = self.join_readers_until(Instant::now() + EXIT_WAIT);
                return match other {
                    Err(error) => Err(error),
                    _ => Err(HarnessError::Protocol(
                        "setup command deadline exceeded".into(),
                    )),
                };
            }
        };
        // child終了後もpipeが保持される場合を、同じsetup deadlineで失敗にする。
        self.join_readers_until(deadline)?;
        let stdout = self
            .stdout
            .lock()
            .map_err(|_| HarnessError::Protocol("setup stdout capture poisoned".into()))?
            .clone();
        let stderr = self
            .stderr
            .lock()
            .map_err(|_| HarnessError::Protocol("setup stderr capture poisoned".into()))?
            .clone();
        if stdout.len() > MAX_COMMAND_OUTPUT_BYTES || stderr.len() > MAX_COMMAND_OUTPUT_BYTES {
            return Err(HarnessError::Protocol(
                "setup command output byte budget exceeded".into(),
            ));
        }
        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
    }

    fn wait_until(&mut self, deadline: Instant) -> Result<Option<ExitStatus>> {
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(Some(status));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            thread::sleep(remaining.min(Duration::from_millis(20)));
        }
    }

    fn kill_and_reap(&mut self, timeout: Duration) -> Result<ExitStatus> {
        if let Ok(Some(status)) = self.child.try_wait() {
            return Ok(status);
        }
        // killと自然終了の競合ではtry_waitで回収を確認する。
        let _ = self.child.kill();
        self.wait_until(Instant::now() + timeout)?.ok_or_else(|| {
            HarnessError::Protocol(format!(
                "{} could not be reaped within {timeout:?}",
                self.name
            ))
        })
    }

    fn shutdown(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let result = (|| {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            let signal_result = unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
            if signal_result != 0 {
                return Err(io_error("send SIGTERM", std::io::Error::last_os_error()));
            }
            self.wait_until(Instant::now() + timeout)?.ok_or_else(|| {
                HarnessError::Protocol(format!("{} did not exit within {timeout:?}", self.name))
            })
        })();
        if result.is_err() {
            let _ = self.kill_and_reap(EXIT_WAIT);
        }
        let readers = self.join_readers_until(Instant::now() + EXIT_WAIT);
        let status = result?;
        readers?;
        self.require_successful_exit(status)
    }

    fn require_successful_exit(&self, status: ExitStatus) -> Result<ExitStatus> {
        if status.success() {
            Ok(status)
        } else {
            Err(HarnessError::Protocol(format!(
                "{} did not complete graceful shutdown: {status}",
                self.name
            )))
        }
    }

    fn join_readers_until(&mut self, deadline: Instant) -> Result<()> {
        let mut failed = false;
        while !self.readers.is_empty() {
            let mut index = 0;
            while index < self.readers.len() {
                if self.readers[index].is_finished() {
                    let reader = self.readers.swap_remove(index);
                    if !matches!(reader.join(), Ok(Ok(()))) {
                        failed = true;
                    }
                } else {
                    index += 1;
                }
            }
            if self.readers.is_empty() {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // stdのblocking Readは取消せない。異常なpipe保持時はthreadをdetachし、
                // 呼出元の待機だけをboundedにする。子孫processの回収までは保証しない。
                self.readers.clear();
                return Err(HarnessError::Protocol(format!(
                    "{} output readers did not finish before deadline",
                    self.name
                )));
            }
            thread::sleep(remaining.min(Duration::from_millis(20)));
        }
        if failed {
            return Err(HarnessError::Protocol(format!(
                "{} output reader failed",
                self.name
            )));
        }
        Ok(())
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        let _ = self.kill_and_reap(EXIT_WAIT);
        let _ = self.join_readers_until(Instant::now() + EXIT_WAIT);
    }
}

fn capture_stream(
    mut stream: impl Read + Send + 'static,
    path: Option<PathBuf>,
    shared: Arc<Mutex<Vec<u8>>>,
    capture_limit: usize,
) -> JoinHandle<std::io::Result<()>> {
    thread::spawn(move || {
        let mut file = path.and_then(|path| File::create(path).ok());
        let mut written = 0usize;
        let mut truncated = false;
        let mut buffer = [0u8; 4096];
        loop {
            let count = match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if let Ok(mut captured) = shared.lock()
                && captured.len() < capture_limit
            {
                let remaining = capture_limit - captured.len();
                captured.extend_from_slice(&buffer[..count.min(remaining)]);
            }
            if let Some(file) = file.as_mut() {
                let remaining = MAX_PROCESS_LOG_BYTES.saturating_sub(written);
                let keep = count.min(remaining);
                if keep > 0 {
                    let _ = file.write_all(&buffer[..keep]);
                    written += keep;
                }
                if keep < count && !truncated {
                    let _ = file.write_all(b"\n<bounded log truncated>\n");
                    truncated = true;
                }
            }
        }
        Ok(())
    })
}

fn parse_address_after(output: &str, marker: &str) -> Option<String> {
    output
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
        .find_map(|line| {
            let rest = line.split_once(marker)?.1;
            let address = rest
                .trim_start()
                .split(|character: char| {
                    character.is_whitespace() || character == ';' || character == '`'
                })
                .next()?
                .trim_start_matches("http://")
                .trim_end_matches('/');
            let socket: std::net::SocketAddr = address.parse().ok()?;
            (socket.port() != 0).then(|| address.to_string())
        })
}

fn assert_absent_or_empty(path: &Path, label: &str) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() && fs::read_dir(path)?.next().is_none() {
        return Ok(());
    }
    Err(HarnessError::Protocol(format!(
        "{label} {} must start empty",
        path.display()
    )))
}

fn require_compatible_connection(connection: &Value) -> Result<()> {
    if connection.get("status").and_then(Value::as_str) != Some("compatible") {
        return Err(HarnessError::Protocol(
            "Runtime connection was not compatible".to_owned(),
        ));
    }
    Ok(())
}

fn require_registered_runtime(runtimes: &Value, runtime_id: &str, binding_id: &str) -> Result<()> {
    let connected = runtimes
        .get("items")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("runtime_id").and_then(Value::as_str) == Some(runtime_id)
                    && item
                        .pointer("/management/binding/binding_id")
                        .and_then(Value::as_str)
                        == Some(binding_id)
            })
        });
    if !connected {
        return Err(HarnessError::Protocol(
            "Runtime registration did not match the verified binding".to_owned(),
        ));
    }
    Ok(())
}

fn require_empty_worker_list(workers: &Value) -> Result<()> {
    if workers
        .get("items")
        .and_then(Value::as_array)
        .is_none_or(|items| !items.is_empty())
    {
        return Err(HarnessError::Protocol(
            "fresh Worker list is not empty or has invalid shape".to_owned(),
        ));
    }
    Ok(())
}

fn json_string(value: &Value, pointer: &'static str) -> Result<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| HarnessError::Protocol(format!("JSON response omitted string at {pointer}")))
}

fn url_port(url: &str) -> Result<u16> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.port())
        .ok_or_else(|| HarnessError::Protocol(format!("URL has no explicit port: {url}")))
}

fn bounded_text(bytes: &[u8], limit: usize) -> String {
    let keep = bytes.len().min(limit);
    let mut text = String::from_utf8_lossy(&bytes[..keep]).into_owned();
    if bytes.len() > keep {
        text.push_str("\n<bounded output truncated>");
    }
    text
}

fn io_error(operation: &str, error: std::io::Error) -> HarnessError {
    HarnessError::Protocol(format!("{operation}: {error}"))
}

#[cfg(test)]
mod startup_address_tests {
    use super::*;

    // 製品binary providerを呼ばず、同じ所有・収集・cleanup経路へ合成childだけを接続する。
    fn synthetic_environment(artifacts_dir: &Path) -> Result<WorkerE2eEnvironment> {
        let temp_root = tempfile::tempdir()?;
        let root = temp_root.path().to_path_buf();
        fs::create_dir_all(artifacts_dir)?;
        Ok(WorkerE2eEnvironment {
            temp_root: Some(temp_root),
            home: root.clone(),
            xdg_data_home: root.clone(),
            xdg_state_home: root.clone(),
            xdg_config_home: root.clone(),
            xdg_runtime_dir: root.clone(),
            server_data_dir: root.join("server"),
            runtime_data_dir: root.join("runtime"),
            server_config: root.join("server.toml"),
            artifacts_dir: artifacts_dir.to_path_buf(),
            workspace_id: None,
            runtime_id: "synthetic-runtime".into(),
            account_handle: "synthetic-account".into(),
            server_url: None,
            runtime_url: None,
            server: None,
            runtime: None,
            runtime_shutdown: WorkerProcessShutdownOutcome::NotStarted,
            server_shutdown: WorkerProcessShutdownOutcome::NotStarted,
            shutdown_error: None,
            runtime_public_identity: None,
            access_token: None,
            client: Client::builder().no_proxy().build().unwrap(),
            binaries: WorkerBinaryProviderInfo {
                workspace_root: root.clone(),
                profile: "synthetic".into(),
                binaries: Vec::new(),
            },
            root,
        })
    }

    fn synthetic_process(root: &Path, name: &str, script: &str) -> Result<ManagedProcess> {
        let mut command = Command::new("/bin/sh");
        command
            .env_clear()
            .current_dir(root)
            .stdin(Stdio::null())
            .args(["-c", script]);
        ManagedProcess::spawn_captured(name, command, None, None, MAX_COMMAND_OUTPUT_BYTES + 1)
    }

    fn assert_secret_free(diagnostic: impl std::fmt::Debug) {
        assert!(!format!("{diagnostic:?}").contains("private-test-marker"));
    }

    #[test]
    fn setup_stdout_is_returned_but_redacted_in_logs_and_nonzero_errors() -> Result<()> {
        let artifacts = tempfile::tempdir()?;
        let environment = synthetic_environment(artifacts.path())?;
        // script自体はsecretの完全値を含めず、stdoutの漏洩だけを検出する。
        let script = "printf 'private-%s' test-marker";
        let output = environment.run_command(Path::new("/bin/sh"), &["-c", script], true)?;
        assert_eq!(output.stdout, b"private-test-marker");
        let error = environment
            .run_command(
                Path::new("/bin/sh"),
                &["-c", "printf 'private-%s' test-marker; exit 7"],
                true,
            )
            .unwrap_err();
        assert!(
            matches!(&error, HarnessError::CommandFailed { status, stdout, .. }
            if status.code() == Some(7) && stdout == "<redacted>")
        );
        assert_secret_free(&error);
        assert_secret_free(error.to_string());
        let log = fs::read_to_string(artifacts.path().join("setup-commands.jsonl"))?;
        assert_secret_free(&log);
        for line in log.lines() {
            let record: Value = serde_json::from_str(line)?;
            assert_eq!(record["stdout"], "<redacted>");
        }
        // setup経路はraw stdout/stderr fileを作らない。
        assert_eq!(fs::read_dir(artifacts.path())?.count(), 1);
        let report = environment.cleanup()?;
        assert!(report.cleanup_success && report.root_cleanup_success);
        Ok(())
    }

    #[test]
    fn setup_deadline_kills_and_reaps_owned_child_without_logging_stdout() -> Result<()> {
        let artifacts = tempfile::tempdir()?;
        let environment = synthetic_environment(artifacts.path())?;
        let script = "trap '' TERM; printf 'private-%s' test-marker; while :; do :; done";
        let timeout = Duration::from_millis(100);
        let mut process = synthetic_process(&environment.root, "setup command", script)?;
        let started = Instant::now();
        let error = process.collect_output(timeout).unwrap_err();
        assert!(error.to_string().contains("deadline exceeded"));
        assert_secret_free(&error);
        assert!(started.elapsed() < timeout + EXIT_WAIT + EXIT_WAIT);
        assert!(process.child.try_wait()?.is_some());
        assert!(process.readers.is_empty());
        let error = environment
            .run_command_with_timeout(Path::new("/bin/sh"), &["-c", script], true, timeout)
            .unwrap_err();
        assert_secret_free(&error);
        let log = fs::read_to_string(artifacts.path().join("setup-commands.jsonl"))?;
        assert_secret_free(&log);
        let record: Value = serde_json::from_str(log.trim())?;
        assert_eq!(record["stdout"], "<redacted>");
        assert!(
            record["error"]
                .as_str()
                .unwrap()
                .contains("deadline exceeded")
        );
        assert_eq!(fs::read_dir(artifacts.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn setup_output_overflow_is_bounded_drained_and_rejected() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut process = synthetic_process(
            root.path(),
            "setup command",
            "i=0; while [ \"$i\" -lt 20000 ]; do printf 0123456789abcdef; printf fedcba9876543210 >&2; i=$((i + 1)); done",
        )?;
        let error = process.collect_output(SETUP_WAIT).unwrap_err();
        assert!(error.to_string().contains("output byte budget exceeded"));
        // 両pipeを上限後もdrainしなければこの成功exitまで到達しない。
        assert!(process.child.try_wait()?.unwrap().success());
        assert!(process.readers.is_empty());
        assert_eq!(
            process.stdout.lock().unwrap().len(),
            MAX_COMMAND_OUTPUT_BYTES + 1
        );
        assert_eq!(
            process.stderr.lock().unwrap().len(),
            MAX_COMMAND_OUTPUT_BYTES + 1
        );
        assert_eq!(fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn setup_output_at_byte_limit_is_returned_without_truncation() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut process = synthetic_process(
            root.path(),
            "setup command",
            "i=0; while [ \"$i\" -lt 4096 ]; do printf 0123456789abcdef; i=$((i + 1)); done",
        )?;
        let output = process.collect_output(SETUP_WAIT)?;
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), MAX_COMMAND_OUTPUT_BYTES);
        assert!(
            output
                .stdout
                .chunks_exact(16)
                .all(|chunk| chunk == b"0123456789abcdef")
        );
        assert!(output.stderr.is_empty());
        Ok(())
    }

    #[test]
    fn failed_output_reader_rejects_successful_child_without_raw_error() -> Result<()> {
        struct FailedReader;
        impl Read for FailedReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("private-test-marker"))
            }
        }
        let root = tempfile::tempdir()?;
        let mut process = synthetic_process(root.path(), "setup command", "exit 0")?;
        process.readers.push(capture_stream(
            FailedReader,
            None,
            Arc::new(Mutex::new(Vec::new())),
            MAX_COMMAND_OUTPUT_BYTES + 1,
        ));
        let error = process.collect_output(SETUP_WAIT).unwrap_err();
        assert!(error.to_string().contains("output reader failed"));
        assert_secret_free(&error);
        assert_secret_free(error.to_string());
        assert!(process.child.try_wait()?.unwrap().success());
        Ok(())
    }

    #[test]
    fn unfinished_output_reader_has_bounded_join_and_is_not_success() -> Result<()> {
        let root = tempfile::tempdir()?;
        let mut process = synthetic_process(root.path(), "setup command", "exit 0")?;
        assert!(
            process
                .wait_until(Instant::now() + EXIT_WAIT)?
                .unwrap()
                .success()
        );
        // 子孫を残さず、pipe保持と同じ未完readerをchannelで決定的に再現する。
        let (release, pending) = std::sync::mpsc::channel::<()>();
        let (finished, completion) = std::sync::mpsc::channel();
        process.readers.push(thread::spawn(move || {
            let _ = pending.recv();
            let _ = finished.send(());
            Ok(())
        }));
        let timeout = Duration::from_millis(50);
        let started = Instant::now();
        let error = process.collect_output(timeout).unwrap_err();
        drop(release);
        completion.recv_timeout(EXIT_WAIT).unwrap();
        assert!(
            error
                .to_string()
                .contains("readers did not finish before deadline")
        );
        assert!(started.elapsed() < timeout + EXIT_WAIT);
        assert!(process.readers.is_empty());
        Ok(())
    }

    #[test]
    fn runtime_failure_still_reaps_live_server_and_survives_repeated_shutdown() -> Result<()> {
        let artifacts = tempfile::tempdir()?;
        let mut environment = synthetic_environment(artifacts.path())?;
        let mut runtime = synthetic_process(&environment.root, "runtime", "exit 7")?;
        assert_eq!(
            runtime
                .wait_until(Instant::now() + EXIT_WAIT)?
                .unwrap()
                .code(),
            Some(7)
        );
        environment.runtime = Some(runtime);
        let server = synthetic_process(
            &environment.root,
            "server",
            "trap 'exit 0' TERM; printf 'ready 127.0.0.1:12345\\n'; while :; do :; done",
        )?;
        server.wait_for_address("ready ", EXIT_WAIT)?;
        environment.server = Some(server);
        for _ in 0..2 {
            assert!(environment.shutdown().is_err());
            assert!(environment.runtime.is_none() && environment.server.is_none());
            assert!(matches!(
                environment.runtime_shutdown,
                WorkerProcessShutdownOutcome::Failed { .. }
            ));
            assert_eq!(
                environment.server_shutdown,
                WorkerProcessShutdownOutcome::Succeeded
            );
        }
        let root = environment.root.clone();
        assert!(environment.cleanup().is_err());
        assert!(!root.exists());
        let report: WorkerEnvironmentCleanupReport =
            serde_json::from_slice(&fs::read(artifacts.path().join("cleanup.json"))?)?;
        assert!(!report.cleanup_success);
        assert!(report.root_cleanup_success);
        assert!(report.cleanup_error.is_some());
        Ok(())
    }

    #[test]
    fn cleanup_and_drop_keep_process_failures_separate_from_root_cleanup() -> Result<()> {
        for explicit_cleanup in [false, true] {
            for (runtime_code, server_code) in [(0, 0), (7, 0), (0, 9), (7, 9)] {
                let artifacts = tempfile::tempdir()?;
                let mut environment = synthetic_environment(artifacts.path())?;
                for (name, code) in [("runtime", runtime_code), ("server", server_code)] {
                    let mut process = synthetic_process(
                        &environment.root,
                        name,
                        &format!("printf 'private-%s' test-marker; exit {code}"),
                    )?;
                    assert_eq!(
                        process
                            .wait_until(Instant::now() + EXIT_WAIT)?
                            .unwrap()
                            .code(),
                        Some(code)
                    );
                    if name == "runtime" {
                        environment.runtime = Some(process);
                    } else {
                        environment.server = Some(process);
                    }
                }
                let root = environment.root.clone();
                let expected_success = runtime_code == 0 && server_code == 0;
                if explicit_cleanup {
                    assert_eq!(environment.cleanup().is_ok(), expected_success);
                } else {
                    drop(environment);
                }
                let raw_report = fs::read_to_string(artifacts.path().join("cleanup.json"))?;
                assert_secret_free(&raw_report);
                let report: WorkerEnvironmentCleanupReport = serde_json::from_str(&raw_report)?;
                assert_eq!(report.cleanup_success, expected_success);
                assert_eq!(report.cleanup_error.is_none(), expected_success);
                assert!(report.root_cleanup_success);
                assert!(report.root_cleanup_error.is_none());
                assert!(!report.fixture_root_exists_after && !root.exists());
                for (outcome, code) in [
                    (report.runtime_shutdown, runtime_code),
                    (report.server_shutdown, server_code),
                ] {
                    if code == 0 {
                        assert_eq!(outcome, WorkerProcessShutdownOutcome::Succeeded);
                    } else {
                        assert!(matches!(
                            outcome,
                            WorkerProcessShutdownOutcome::Failed { .. }
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn shutdown_rejects_nonzero_exit_even_when_process_already_exited() -> super::Result<()> {
        let root = tempfile::tempdir()?;
        for code in [0, 7] {
            let mut command = std::process::Command::new("/bin/sh");
            command
                .env_clear()
                .current_dir(root.path())
                .args(["-c", &format!("exit {code}")]);
            let mut process = super::ManagedProcess::spawn(
                "already-exited",
                command,
                &root.path().join(format!("{code}.stdout")),
                &root.path().join(format!("{code}.stderr")),
            )?;
            // Reap first to deterministically exercise the initial try_wait branch.
            assert_eq!(process.child.wait()?.code(), Some(code));
            let result = process.shutdown(std::time::Duration::from_secs(1));
            if code == 0 {
                assert!(result?.success());
            } else {
                assert!(matches!(result, Err(super::HarnessError::Protocol(message))
                    if message.contains("did not complete graceful shutdown")));
            }
        }
        Ok(())
    }

    #[test]
    fn startup_address_waits_for_complete_valid_socket_record() {
        let record = "yoi-server listening on http://127.0.0.1:49123\n";
        for boundary in 0..record.len() {
            assert_eq!(parse_address_after(&record[..boundary], "on http://"), None);
        }
        assert_eq!(
            parse_address_after(record, "on http://").as_deref(),
            Some("127.0.0.1:49123")
        );
        assert_eq!(
            parse_address_after("on http://[::1]:12345\n", "on http://").as_deref(),
            Some("[::1]:12345")
        );
        for invalid in [
            "127",
            "127.0.0.1",
            "127.0.0.1:0",
            "hostname:123",
            "127.0.0.1:99999",
        ] {
            assert_eq!(
                parse_address_after(&format!("on http://{invalid}\n"), "on http://"),
                None
            );
        }
    }
}

#[cfg(test)]
mod response_diagnostic_tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn assert_secret_free(error: &HarnessError) {
        for diagnostic in [error.to_string(), format!("{error:?}")] {
            assert!(!diagnostic.contains("private-test-marker"), "{diagnostic}");
        }
        assert!(std::error::Error::source(error).is_none());
    }

    #[test]
    fn json_decode_errors_do_not_retain_rejected_fields_values_or_enum_names() {
        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Document {
            #[allow(dead_code)]
            count: u64,
        }
        for raw in [
            r#"{"count":"private-test-marker"}"#,
            r#"{"count":1,"private-test-marker":true}"#,
        ] {
            let error: HarnessError = serde_json::from_str::<Document>(raw).unwrap_err().into();
            assert_secret_free(&error);
        }
        let raw = serde_json::json!({"kind":"private-test-marker"});
        let error: HarnessError = serde_json::from_value::<protocol::Event>(raw)
            .unwrap_err()
            .into();
        assert_secret_free(&error);
    }

    #[test]
    fn valid_json_contract_failures_do_not_retain_response_fields_or_values() -> Result<()> {
        let response: Value = serde_json::from_str(
            r#"{"status":"private-test-marker","items":[{"private-test-marker":"private-test-marker"}],"binding_id":{ "private-test-marker":true }}"#,
        )?;
        for error in [
            require_compatible_connection(&response).unwrap_err(),
            require_registered_runtime(&response, "runtime-test", "binding-test").unwrap_err(),
            require_empty_worker_list(&response).unwrap_err(),
            json_string(&response, "/binding_id").unwrap_err(),
            json_string(&response, "/workspace/workspace_id").unwrap_err(),
        ] {
            assert_secret_free(&error);
        }
        require_compatible_connection(&json!({"status":"compatible"}))?;
        require_registered_runtime(
            &json!({"items":[{"runtime_id":"runtime-test","management":{"binding":{"binding_id":"binding-test"}}}]}),
            "runtime-test",
            "binding-test",
        )?;
        require_empty_worker_list(&json!({"items":[]}))?;
        assert_eq!(
            json_string(&json!({"id":"worker-test"}), "/id")?,
            "worker-test"
        );
        Ok(())
    }

    #[test]
    fn http_failure_bodies_and_malformed_success_bodies_are_not_diagnostics() -> Result<()> {
        for (status, body) in [
            ("400 Bad Request", "private-test-marker"),
            ("200 OK", "private-test-marker"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let address = listener.local_addr()?;
            let server = thread::spawn(move || -> std::io::Result<()> {
                let (mut connection, _) = listener.accept()?;
                connection.set_read_timeout(Some(Duration::from_secs(3)))?;
                let mut request = [0; 4096];
                connection.read(&mut request)?;
                write!(
                    connection,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )?;
                Ok(())
            });
            let response = Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap()
                .get(format!("http://{address}"))
                .send()
                .unwrap();
            let error = decode_api_response(response, "test response").unwrap_err();
            server.join().unwrap()?;
            assert_secret_free(&error);
            if status.starts_with("400") {
                assert!(error.to_string().contains("HTTP 400"));
            }
        }
        Ok(())
    }
}
