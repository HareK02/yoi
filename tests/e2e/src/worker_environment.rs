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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerEnvironmentCleanupReport {
    pub fixture_root: PathBuf,
    pub artifacts_dir: PathBuf,
    pub cleanup_attempted: bool,
    pub cleanup_success: bool,
    pub fixture_root_exists_after: bool,
    pub cleanup_error: Option<String>,
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
        if let Some(mut server) = self.server.take() {
            server.shutdown(EXIT_WAIT)?;
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
        let runtime_identity = self
            .runtime_public_identity
            .clone()
            .expect("initialized identity");
        let bundle_path = self.root.join("workspace-public-identity.json");
        let server_url = self.required_server_url()?.to_string();
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
        if connection.get("status").and_then(Value::as_str) != Some("compatible") {
            return Err(HarnessError::Protocol(format!(
                "Runtime connection was not compatible: {connection}"
            )));
        }
        let binding_id = json_string(&connection, "/binding_id")?;
        let runtimes = self.request_json(
            self.client
                .get(format!("{server_url}/api/w/{workspace_id}/runtimes")),
            "list Runtimes",
        )?;
        let connected = runtimes
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("runtime_id").and_then(Value::as_str) == Some(self.runtime_id.as_str())
                        && item
                            .pointer("/management/binding/binding_id")
                            .and_then(Value::as_str)
                            == Some(binding_id.as_str())
                })
            });
        if !connected {
            return Err(HarnessError::Protocol(format!(
                "Runtime {} is not verified in {runtimes}",
                self.runtime_id
            )));
        }
        Ok(())
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
        if workers
            .get("items")
            .and_then(Value::as_array)
            .is_none_or(|items| !items.is_empty())
        {
            return Err(HarnessError::Protocol(format!(
                "fresh Worker list is not empty: {workers}"
            )));
        }
        Ok(())
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
        if let Some(mut runtime) = self.runtime.take() {
            runtime.shutdown(EXIT_WAIT)?;
        }
        if let Some(mut server) = self.server.take() {
            server.shutdown(EXIT_WAIT)?;
        }
        self.write_metadata("stopped")?;
        Ok(())
    }

    pub fn cleanup(mut self) -> Result<WorkerEnvironmentCleanupReport> {
        self.shutdown()?;
        self.cleanup_inner(true)
    }

    fn cleanup_inner(&mut self, strict: bool) -> Result<WorkerEnvironmentCleanupReport> {
        let mut cleanup_error = None;
        if let Some(temp_root) = self.temp_root.take()
            && let Err(error) = temp_root.close()
        {
            cleanup_error = Some(error.to_string());
        }
        let fixture_root_exists_after = self.root.exists();
        let cleanup_success = cleanup_error.is_none() && !fixture_root_exists_after;
        let report = WorkerEnvironmentCleanupReport {
            fixture_root: self.root.clone(),
            artifacts_dir: self.artifacts_dir.clone(),
            cleanup_attempted: true,
            cleanup_success,
            fixture_root_exists_after,
            cleanup_error,
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
        let status = response.status();
        let body = response
            .bytes()
            .map_err(|error| HarnessError::Protocol(format!("{operation}: {error}")))?;
        self.append_operation(operation, status.as_u16())?;
        if !status.is_success() {
            return Err(HarnessError::Protocol(format!(
                "{operation} returned {status}: {}",
                bounded_text(&body, MAX_COMMAND_OUTPUT_BYTES)
            )));
        }
        serde_json::from_slice(&body).map_err(HarnessError::from)
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
        let args_owned = args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>();
        let mut command = self.isolated_command(binary);
        let output = command.args(args).output()?;
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
                "status": output.status.code(),
                "stdout": if redact_stdout {
                    "<redacted>".to_string()
                } else {
                    bounded_text(&output.stdout, MAX_COMMAND_OUTPUT_BYTES)
                },
                "stderr": bounded_text(&output.stderr, MAX_COMMAND_OUTPUT_BYTES),
                "env_clear": true,
            }),
        )?;
        writeln!(log)?;
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

impl Drop for WorkerE2eEnvironment {
    fn drop(&mut self) {
        if let Some(mut runtime) = self.runtime.take() {
            let _ = runtime.shutdown(EXIT_WAIT);
        }
        if let Some(mut server) = self.server.take() {
            let _ = server.shutdown(EXIT_WAIT);
        }
        if self.temp_root.is_some() {
            let _ = self.cleanup_inner(false);
        }
    }
}

#[derive(Debug)]
struct ManagedProcess {
    name: String,
    child: Child,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    readers: Vec<JoinHandle<()>>,
}

impl ManagedProcess {
    fn spawn(
        name: &str,
        mut command: Command,
        stdout_path: &Path,
        stderr_path: &Path,
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
            capture_stream(stdout_pipe, stdout_path.to_path_buf(), Arc::clone(&stdout)),
            capture_stream(stderr_pipe, stderr_path.to_path_buf(), Arc::clone(&stderr)),
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

    fn shutdown(&mut self, timeout: Duration) -> Result<ExitStatus> {
        if let Some(status) = self.child.try_wait()? {
            self.join_readers();
            return self.require_successful_exit(status);
        }
        let signal_result = unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        if signal_result != 0 {
            return Err(io_error("send SIGTERM", std::io::Error::last_os_error()));
        }
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                self.join_readers();
                return self.require_successful_exit(status);
            }
            if start.elapsed() >= timeout {
                let _ = self.child.kill();
                let _ = self.child.wait();
                self.join_readers();
                return Err(HarnessError::Protocol(format!(
                    "{} did not exit within {timeout:?}",
                    self.name
                )));
            }
            thread::sleep(Duration::from_millis(20));
        }
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

    fn join_readers(&mut self) {
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.join_readers();
    }
}

fn capture_stream(
    mut stream: impl Read + Send + 'static,
    path: PathBuf,
    shared: Arc<Mutex<Vec<u8>>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut file = File::create(path).ok();
        let mut written = 0usize;
        let mut truncated = false;
        let mut buffer = [0u8; 4096];
        loop {
            let count = match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            if let Ok(mut captured) = shared.lock()
                && captured.len() < MAX_PROCESS_LOG_BYTES
            {
                let remaining = MAX_PROCESS_LOG_BYTES - captured.len();
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

fn json_string(value: &Value, pointer: &str) -> Result<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| HarnessError::Protocol(format!("JSON response omitted {pointer}: {value}")))
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
    use super::parse_address_after;

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
