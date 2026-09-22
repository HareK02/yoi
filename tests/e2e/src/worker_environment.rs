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
    FixtureAllocation, HarnessError, Result, allocate_fixture, command_display,
    current_target_profile_dir, now_ms, test_profile, workspace_root, yoi_binary_info,
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
    let yoi = yoi_binary_info()?;
    let mut records = vec![binary_record(
        "yoi",
        yoi.binary,
        yoi.provider,
        yoi.build_command,
    )?];

    let server_override = resolve_override("YOI_E2E_SERVER_BIN", &workspace_root)?;
    let runtime_override = resolve_override("YOI_E2E_RUNTIME_BIN", &workspace_root)?;
    let mut build_command = None;
    if server_override.is_none() || runtime_override.is_none() {
        let cargo = PathBuf::from(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        let mut args = vec![
            "build".to_string(),
            "-p".to_string(),
            "yoi-workspace-server".to_string(),
            "--bin".to_string(),
            "yoi-server".to_string(),
            "-p".to_string(),
            "worker-runtime".to_string(),
            "--bin".to_string(),
            "yoi-runtime".to_string(),
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

    let profile_dir = current_target_profile_dir()?;
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

    pub fn start_backend(&mut self) -> Result<()> {
        assert_absent_or_empty(&self.server_data_dir, "Server data directory")?;
        assert_absent_or_empty(&self.runtime_data_dir, "Runtime data directory")?;
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
        self.workspace_id = Some(workspace_id.clone());
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
        self.write_metadata("backend-running")?;
        Ok(())
    }

    fn start_runtime(&mut self) -> Result<()> {
        let bundle_path = self.root.join("workspace-public-identity.json");
        let server_url = self.required_server_url()?.to_string();
        let workspace_id = self.required_workspace_id()?.to_string();
        let runtime_binary = self.binaries.binary("yoi-runtime")?;
        self.run_command(
            &runtime_binary,
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
        let runtime_identity = self.run_command(
            &runtime_binary,
            &[
                "identity",
                "show",
                "--json",
                "--fs-root",
                self.runtime_data_dir.to_string_lossy().as_ref(),
            ],
            false,
        )?;
        let runtime_identity: Value = serde_json::from_slice(&runtime_identity.stdout)?;
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

        self.request_json(
            self.client
                .post(format!("{server_url}/api/w/{workspace_id}/runtimes"))
                .json(&serde_json::json!({
                    "public_bundle": runtime_identity,
                    "display_name": format!("E2E {}", self.runtime_id),
                    "endpoint": runtime_url,
                    "expected_revision": null
                })),
            "register Runtime",
        )?;
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
        self.assert_connected_and_empty()?;
        self.write_metadata("running")?;
        Ok(())
    }

    pub fn assert_connected_and_empty(&self) -> Result<()> {
        let server_url = self.required_server_url()?;
        let workspace_id = self.required_workspace_id()?;
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
                            .pointer("/management/binding/connection_state")
                            .and_then(Value::as_str)
                            == Some("verified")
                })
            });
        if !connected {
            return Err(HarnessError::Protocol(format!(
                "Runtime {} is not verified in {runtimes}",
                self.runtime_id
            )));
        }
        let workers = self.request_json(
            self.client
                .get(format!("{server_url}/api/w/{workspace_id}/workers")),
            "list Workers",
        )?;
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
                    "workflow_state": "planning",
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
            let output = self.output();
            if let Some(address) = parse_address_after(&output, marker) {
                return Ok(address);
            }
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
            return Ok(status);
        }
        let signal_result = unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        if signal_result != 0 {
            return Err(io_error("send SIGTERM", std::io::Error::last_os_error()));
        }
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                self.join_readers();
                if status.success() {
                    return Ok(status);
                }
                return Err(HarnessError::Protocol(format!(
                    "{} did not complete graceful shutdown: {status}",
                    self.name
                )));
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
    let rest = output.split(marker).nth(1)?;
    let address = rest
        .trim_start()
        .split(|character: char| character.is_whitespace() || character == ';' || character == '`')
        .next()?
        .trim_start_matches("http://")
        .trim_end_matches('/');
    (!address.is_empty()).then(|| address.to_string())
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
