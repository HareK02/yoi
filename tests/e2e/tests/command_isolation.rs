//! Opt-in product E2E: real featureless Server/Runtime/yoi children from the shared
//! WorkerE2eEnvironment, isolated DB/keys/HOME/ports, no model input. Runtime case
//! uses freshly signed HTTP requests and real materialization/session admission.
//! External case uses the real grant-creating foreground CLI, WebSocket registration,
//! Backend Worker-source authentication, attachment and command broker. The driver
//! signs with this fixture's fresh keys; it does not test a model's tool selection.
//! No sessions, grants, providers or command results are pre-seeded. Requires Linux,
//! cgroup v2, systemd user manager and common user@.service OOMPolicy=continue. Only
//! 32MiB command units may OOM. Never change parent/host policy or use live data.
//! Run with explicit e2e feature, normal product binaries in a separate target,
//! outer finite memory/time budget and --test-threads=1. This does not validate
//! tmux pane regeneration or deployment of the operator's drop-in configuration.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use workdir::http::{
    OpenWorkdirSessionRequest, WorkdirSessionOperation as Op, WorkdirSessionOperationResponse,
    WorkdirSessionOperationResult as OpResult,
};
use workdir::{
    CommandHandle, CommandOomObservation, CommandOutputRequest, CommandResourceOutcome,
    CommandStatus, CommandTerminal,
};
use worker_runtime::auth::{RuntimeIdentityMaterial, RuntimeRequestSourceSigner};
use worker_runtime::catalog::{
    RepositoryMaterializationContext, WorkingDirectoryRepository, WorkingDirectoryRequest,
};
use worker_runtime::workspace_issuer::{WorkspaceCapabilityClaims, workspace_request_body_digest};
use yoi_e2e::{HarnessError, Result, WorkerE2eEnvironment};
use yoi_workspace_server::workspace_signing_identity::{
    FsWorkspaceSigningMaterialStore, WorkspaceSigningIdentityService,
    workspace_signing_material_root,
};

fn failure(message: &str) -> HarnessError {
    HarnessError::Protocol(message.into())
}
fn field(value: &Value, pointer: &str) -> Result<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| failure("product response omitted a required identifier"))
}
fn decode(response: reqwest::blocking::Response) -> Result<Value> {
    let status = response.status();
    if !status.is_success() {
        return Err(failure(&format!(
            "isolation product HTTP status {}",
            status.as_u16()
        )));
    }
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(response, 65537), &mut bytes)?;
    if bytes.len() > 65536 {
        return Err(failure("product response exceeds budget"));
    }
    serde_json::from_slice(&bytes).map_err(HarnessError::from)
}

struct SignedRuntime<'a> {
    environment: &'a WorkerE2eEnvironment,
    signer: WorkspaceSigningIdentityService,
    issuer: String,
    client: Client,
}
impl<'a> SignedRuntime<'a> {
    fn new(environment: &'a WorkerE2eEnvironment) -> Result<Self> {
        let db = environment.server_data_dir.join("server.db");
        let store = Arc::new(
            yoi_workspace_server::SqliteWorkspaceStore::open(&db)
                .map_err(|_| failure("open owned fixture store"))?,
        );
        let signer = WorkspaceSigningIdentityService::new(
            store,
            Arc::new(FsWorkspaceSigningMaterialStore::new(
                workspace_signing_material_root(&db),
            )),
        );
        let bundle: server_api::WorkspacePublicIdentityBundle = serde_json::from_slice(&fs::read(
            environment.root.join("workspace-public-identity.json"),
        )?)?;
        Ok(Self {
            environment,
            signer,
            issuer: bundle.backend_url,
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|_| failure("build Runtime client"))?,
        })
    }
    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        let response = self.send(method, path, body, false)?;
        eprintln!("Runtime {method} {path}: {}", response.status());
        decode(response)
    }
    fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        change_signed_body: bool,
    ) -> Result<reqwest::blocking::Response> {
        let environment = self.environment;
        let workspace = environment
            .workspace_id
            .as_deref()
            .ok_or_else(|| failure("missing fixture Workspace"))?;
        let identity = self
            .signer
            .get_validated(workspace)
            .map_err(|_| failure("read fixture signing identity"))?;
        let bytes = body
            .map(serde_json::to_vec)
            .transpose()?
            .unwrap_or_default();
        let now = chrono::Utc::now().timestamp();
        let claims = WorkspaceCapabilityClaims {
            issuer: self.issuer.clone(),
            issuer_workspace_id: workspace.into(),
            issuer_key_id: identity.key_id,
            issuer_public_key_fingerprint: identity
                .public_key_fingerprint
                .ok_or_else(|| failure("missing signing fingerprint"))?,
            runtime_id: environment.runtime_id.clone(),
            worker_id: None,
            operation: if path.starts_with("/v1/workdir-sessions")
                || (path.starts_with("/v1/working-directories/") && path.ends_with("/sessions"))
            {
                "workdirs:operate"
            } else {
                "workers:create"
            }
            .into(),
            method: method.into(),
            path_and_query: path.into(),
            body_digest: workspace_request_body_digest(&bytes),
            iat: now,
            exp: now + 30,
            jti: uuid::Uuid::now_v7().to_string(),
        };
        let token = self
            .signer
            .issue_workspace_capability(workspace, &claims)
            .map_err(|_| failure("sign fixture Runtime request"))?;
        let base = environment
            .runtime_url
            .as_deref()
            .ok_or_else(|| failure("Runtime not started"))?;
        let mut transmitted = bytes;
        if change_signed_body {
            transmitted.push(b' ');
        }
        self.client
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| failure("method"))?,
                format!("{base}{path}"),
            )
            .bearer_auth(token)
            .header("content-type", "application/json")
            .body(transmitted)
            .send()
            .map_err(|_| failure("Runtime transport failed, do not resend"))
    }
    fn session(&self, workdir: &str) -> Result<RuntimeSession<'_, 'a>> {
        let request = OpenWorkdirSessionRequest::default();
        let id = request.session_id.as_str().to_owned();
        // Own the requested identity BEFORE dispatch: Drop also closes unknown opens.
        let session = RuntimeSession {
            runtime: self,
            id,
            open: true,
        };
        let response = self.request(
            "POST",
            &format!("/v1/working-directories/{workdir}/sessions"),
            Some(&serde_json::to_value(request)?),
        )?;
        assert_eq!(field(&response, "/session_id")?, session.id);
        assert_eq!(field(&response, "/workdir_id")?, workdir);
        Ok(session)
    }
}

trait Peer {
    fn operation(&mut self, operation: Op) -> Result<OpResult>;
}
struct RuntimeSession<'a, 'b> {
    runtime: &'a SignedRuntime<'b>,
    id: String,
    open: bool,
}
impl RuntimeSession<'_, '_> {
    fn close(mut self) -> Result<()> {
        self.runtime
            .request("DELETE", &format!("/v1/workdir-sessions/{}", self.id), None)?;
        self.open = false;
        Ok(())
    }
}
impl Drop for RuntimeSession<'_, '_> {
    fn drop(&mut self) {
        if self.open {
            let _ =
                self.runtime
                    .request("DELETE", &format!("/v1/workdir-sessions/{}", self.id), None);
        }
    }
}
impl Peer for RuntimeSession<'_, '_> {
    fn operation(&mut self, operation: Op) -> Result<OpResult> {
        let value = self.runtime.request(
            "POST",
            &format!("/v1/workdir-sessions/{}/operations", self.id),
            Some(&json!({"operation": operation})),
        )?;
        Ok(serde_json::from_value(value)?)
    }
}

struct ExternalPeer<'a> {
    environment: &'a WorkerE2eEnvironment,
    signer: RuntimeRequestSourceSigner,
    worker: String,
    client: Client,
    audience: String,
}
impl ExternalPeer<'_> {
    fn request(&self, method: &str, suffix: &str, body: Option<&Value>) -> Result<Value> {
        let (base, workspace) = self.environment.backend_target()?;
        let path = format!("/api/w/{workspace}/workers/self/{suffix}");
        let bytes = body
            .map(serde_json::to_vec)
            .transpose()?
            .unwrap_or_default();
        let proof = self
            .signer
            .issue(
                &self.audience,
                workspace,
                Some(&self.worker),
                worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                method,
                &path,
                &bytes,
                chrono::Utc::now().timestamp(),
                30,
            )
            .map_err(|_| failure("sign fixture Worker request"))?;
        decode(
            self.client
                .request(
                    reqwest::Method::from_bytes(method.as_bytes())
                        .map_err(|_| failure("method"))?,
                    format!("{base}{path}"),
                )
                .header(
                    worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                    proof,
                )
                .header("content-type", "application/json")
                .body(bytes)
                .send()
                .map_err(|_| failure("Backend transport failed, do not resend"))?,
        )
    }
}
impl Peer for ExternalPeer<'_> {
    fn operation(&mut self, operation: Op) -> Result<OpResult> {
        let value = self.request(
            "POST",
            "workdir-session/operations",
            Some(&json!({"target_workdir":"external", "operation": operation})),
        )?;
        let response: WorkdirSessionOperationResponse = serde_json::from_value(value)?;
        assert!(
            response.audit_failures.is_empty(),
            "command audit must be retained"
        );
        Ok(response.result)
    }
}

fn start(peer: &mut impl Peer, script: &str) -> Result<CommandHandle> {
    let request = serde_json::from_value(
        json!({"command":script,"timeout_secs":15,"transport_byte_budget":65536,"cwd":"."}),
    )?;
    match peer.operation(Op::CommandStart(request))? {
        OpResult::CommandStart(handle) => Ok(handle),
        _ => Err(failure("wrong command-start result")),
    }
}
fn collect(peer: &mut impl Peer, handle: CommandHandle) -> Result<(CommandTerminal, Vec<u8>)> {
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut cursor = 0;
    let mut stdout = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err(failure("command output deadline; command is not retried"));
        }
        let OpResult::CommandOutput(page) =
            peer.operation(Op::CommandOutput(CommandOutputRequest {
                handle: handle.clone(),
                cursor,
                limit: 65536,
                wait: false,
            }))?
        else {
            return Err(failure("wrong command-output result"));
        };
        for chunk in page.chunks {
            if chunk.stream == workdir::CommandStream::Stdout {
                stdout.extend(chunk.bytes);
            }
        }
        assert!(stdout.len() <= 65536);
        cursor = page.next_cursor;
        if let Some(terminal) = page.terminal {
            eprintln!(
                "command terminal: {terminal:?}; retained stdout: {:?}",
                String::from_utf8_lossy(&stdout)
            );
            return Ok((terminal, stdout));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn expect_complete(terminal: &CommandTerminal) {
    assert!(
        terminal.metadata_valid()
            && terminal.output_collection == workdir::CommandOutputCollection::Complete,
        "output transport must reach its real terminal"
    );
    assert!(
        matches!(
            terminal.resource_outcome,
            Some(CommandResourceOutcome::Systemd {
                cleanup_confirmed: true,
                ..
            })
        ),
        "isolated command must confirm unit cleanup"
    );
}
fn exercise(affected: &mut impl Peer, independent: &mut impl Peer) -> Result<()> {
    let sibling = start(
        independent,
        "printf independent-before; while ! test -f independent-release; do sleep 0.02; done; printf independent-after",
    )?;
    // Confirm the fixed, small limit in the command itself before attempting OOM.
    // One shell child allocates >32MiB; it is never a host-global pressure test.
    let oom = start(
        affected,
        "read -r cg </proc/self/cgroup; read -r limit <\"/sys/fs/cgroup${cg#0::}/memory.max\"; test \"$limit\" = 33554432 || exit 125; printf 'scope=%s\\nretained-prefix' \"${cg#0::}\"; bash -c 'x=$(head -c 67108864 /dev/zero | tr \"\\0\" x)'",
    )?;
    let (terminal, bytes) = collect(affected, oom)?;
    expect_complete(&terminal);
    assert_eq!(terminal.status, CommandStatus::Failed);
    assert!(matches!(
        terminal.resource_outcome,
        Some(CommandResourceOutcome::Systemd {
            oom: CommandOomObservation::SystemdOomKill,
            cleanup_confirmed: true
        })
    ));
    let output = std::str::from_utf8(&bytes).map_err(|_| failure("scope output encoding"))?;
    let (scope_line, prefix) = output
        .split_once('\n')
        .ok_or_else(|| failure("scope observation missing"))?;
    let scope = scope_line
        .strip_prefix("scope=/")
        .ok_or_else(|| failure("scope observation missing"))?;
    assert!(scope.contains("/app.slice/yoi-command-") && scope.ends_with(".scope"));
    assert_eq!(prefix, "retained-prefix");
    assert!(
        !PathBuf::from("/sys/fs/cgroup").join(scope).exists(),
        "the OOM cgroup must actually be removed"
    );
    assert_eq!(
        independent.operation(Op::CommandStatus(sibling.clone()))?,
        OpResult::CommandStatus(CommandStatus::Running)
    );
    let release = start(independent, "touch independent-release")?;
    let (released, _) = collect(independent, release)?;
    expect_complete(&released);
    assert_eq!(released.status, CommandStatus::Completed);
    let (terminal, bytes) = collect(independent, sibling)?;
    expect_complete(&terminal);
    assert_eq!(terminal.status, CommandStatus::Completed);
    assert_eq!(bytes, b"independent-beforeindependent-after");
    for (script, expected) in [
        ("printf after-oom", Some(&b"after-oom"[..])),
        ("kill -KILL $$", None),
        ("exit 137", None),
    ] {
        let handle = start(affected, script)?;
        let (terminal, bytes) = collect(affected, handle)?;
        expect_complete(&terminal);
        assert!(!matches!(
            terminal.resource_outcome,
            Some(CommandResourceOutcome::Systemd {
                oom: CommandOomObservation::SystemdOomKill,
                ..
            })
        ));
        if let Some(expected) = expected {
            assert_eq!(terminal.status, CommandStatus::Completed);
            assert_eq!(bytes, expected);
        } else {
            assert_eq!(terminal.status, CommandStatus::Failed);
        }
    }
    Ok(())
}

fn executable(name: &str) -> Result<PathBuf> {
    let path = std::env::var_os("PATH").ok_or_else(|| failure("PATH unavailable"))?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| failure("required executable unavailable"))
}
fn create_repository(environment: &WorkerE2eEnvironment) -> Result<PathBuf> {
    let root = environment.root.join("repository");
    fs::create_dir(&root)?;
    let git = executable("git")?;
    let path = root
        .to_str()
        .ok_or_else(|| failure("repository path encoding"))?;
    assert!(
        environment
            .run_cli(&git, &["-C", path, "init", "-b", "develop"])?
            .status
            .success()
    );
    assert!(
        environment
            .run_cli(
                &git,
                &[
                    "-C",
                    path,
                    "-c",
                    "user.name=E2E",
                    "-c",
                    "user.email=e2e@example.invalid",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "isolation fixture"
                ]
            )?
            .status
            .success()
    );
    Ok(root)
}

#[test]
#[ignore = "requires user systemd/cgroup v2, continue-policy common ancestor; normal product binaries and bounded outer scope"]
fn runtime_signed_and_external_cli_commands_survive_each_others_oom() -> Result<()> {
    // systemd-run's local user transport uses XDG_RUNTIME_DIR/systemd/private,
    // not DBUS_SESSION_BUS_ADDRESS. Keep an owned, short Unix-socket path and
    // expose only that manager endpoint, never the real Runtime's data directory.
    let manager_runtime = tempfile::Builder::new()
        .prefix("yoi-systemd-")
        .tempdir_in("/tmp")?;
    fs::create_dir(manager_runtime.path().join("systemd"))?;
    std::os::unix::fs::symlink(
        format!("/run/user/{}/systemd/private", unsafe { libc::geteuid() }),
        manager_runtime.path().join("systemd/private"),
    )?;
    let mut environment = WorkerE2eEnvironment::new()?;
    environment.xdg_runtime_dir = manager_runtime.path().to_owned();
    // Explicit tool lookup and user-manager access; HOME/data/credentials and
    // application runtime files remain fixture-owned. No global env mutation.
    let path = std::env::var_os("PATH").ok_or_else(|| failure("PATH unavailable"))?;
    let bus = format!("unix:path=/run/user/{}/bus", unsafe { libc::geteuid() });
    environment.use_systemd_command_host(path, bus.into())?;
    environment.start()?;
    let repository = create_repository(&environment)?;
    let runtime = SignedRuntime::new(&environment)?;
    let source = yoi_workspace_server::repository_source::parse_repository_source(
        repository
            .to_str()
            .ok_or_else(|| failure("repository encoding"))?,
    )
    .map_err(|_| failure("local repository source"))?;
    let id = format!("isolation-{}", uuid::Uuid::now_v7().simple());
    let request = WorkingDirectoryRequest {
        repository: WorkingDirectoryRepository {
            id: "isolation-fixture".into(),
            provider: "git".into(),
            source_fingerprint:
                yoi_workspace_server::repository_source::repository_source_fingerprint(&source),
            source,
            selector: Some("develop".into()),
        },
        display_name: Some("OOM fixture".into()),
        materializer: Default::default(),
        backend_workdir_id: Some(id.clone()),
        materialization: Some(RepositoryMaterializationContext {
            workspace_id: environment
                .workspace_id
                .clone()
                .ok_or_else(|| failure("Workspace"))?,
            runtime_id: environment.runtime_id.clone(),
            operation_id: uuid::Uuid::now_v7().to_string(),
            config_projection_digest: "command-isolation-fixture".into(),
            ssh: None,
        }),
    };
    runtime.request(
        "POST",
        "/v1/working-directories",
        Some(&serde_json::to_value(request)?),
    )?;
    let mut runtime_session = runtime.session(&id)?;
    let rejected = runtime.send(
        "POST",
        &format!("/v1/workdir-sessions/{}/operations", runtime_session.id),
        Some(&json!({"operation": Op::CommandStatus(CommandHandle("never-created".into()))})),
        true,
    )?;
    assert!(
        matches!(rejected.status().as_u16(), 401 | 403),
        "signed body tampering must be rejected before dispatch"
    );

    let (base, workspace) = environment.backend_target()?;
    environment.install_backend_token(&environment.xdg_config_home)?;
    let external_root = environment.root.join("external");
    fs::create_dir(&external_root)?;
    let workspace_name = format!("Worker E2E {}", environment.runtime_id);
    let mut share = environment.start_product_cli(
        "command-share",
        &[
            "--backend",
            base,
            "workdir",
            "share",
            external_root
                .to_str()
                .ok_or_else(|| failure("external root encoding"))?,
            "--workspace-name",
            &workspace_name,
            "--permission",
            "command",
            "--ttl",
            "120s",
            "--display-name",
            "isolation-external",
            "--non-interactive",
        ],
    )?;
    let registry_path = format!("/api/w/{workspace}/working-directories");
    let deadline = Instant::now() + Duration::from_secs(15);
    let (external_id, grant_id) = loop {
        share.assert_running()?;
        let registry = environment.request_backend_json("GET", &registry_path, None)?;
        if let Some(item) = registry["items"].as_array().and_then(|items| {
            items
                .iter()
                .find(|item| item["display_name"] == "isolation-external")
        }) {
            let grant_id = field(item, "/source/grant_id")?;
            let grant = environment.request_backend_json(
                "GET",
                &format!("/api/w/{workspace}/external-workdir-grants/{grant_id}"),
                None,
            )?;
            if grant["status"] == "online" {
                break (field(&grant, "/working_directory_id")?, grant_id);
            }
        }
        if Instant::now() >= deadline {
            return Err(failure("External CLI registration deadline"));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let worker = environment.request_backend_json("POST", &format!("/api/w/{workspace}/runtimes/{}/workers",environment.runtime_id), Some(&json!({
        "control_operation_id":uuid::Uuid::now_v7().to_string(),"intent":{"kind":"workspace_coding"},"requested_worker_name":"Command isolation driver","acceptance":{"kind":"run_accepted","expected_segments":0},"initial_submit":[],"profile":{"kind":"builtin","value":"builtin:coder"}
    })))?;
    eprintln!(
        "Worker create state: {}; diagnostic codes: {:?}",
        worker["state"],
        worker["diagnostics"]
            .as_array()
            .map(|items| items.iter().map(|item| &item["code"]).collect::<Vec<_>>())
    );
    let worker_id = field(&worker, "/worker/worker_id")?;
    // Read only this environment's freshly generated Runtime identity. No stored
    // authentication success, sessions or authority decisions are substituted.
    #[derive(Deserialize)]
    struct Auth {
        identity: RuntimeIdentityMaterial,
    }
    let auth: Auth = toml::from_str(&fs::read_to_string(
        environment.runtime_data_dir.join("runtime/auth.toml"),
    )?)
    .map_err(|_| failure("decode owned Runtime identity"))?;
    let mut external = ExternalPeer {
        environment: &environment,
        signer: RuntimeRequestSourceSigner::from_identity(&auth.identity),
        worker: worker_id.clone(),
        audience: runtime.issuer.clone(),
        client: Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| failure("build Backend driver"))?,
    };
    let attached = external.request(
        "POST",
        "workdir-attachments",
        Some(&json!({"alias":"external","working_directory_id":external_id})),
    )?;
    assert_eq!(attached["attached"], true);
    exercise(&mut runtime_session, &mut external)?;
    environment.assert_connected()?;
    share.assert_running()?;
    exercise(&mut external, &mut runtime_session)?;
    environment.assert_connected()?;
    share.assert_running()?;
    external.request("DELETE", "workdir-attachments/external", None)?;
    runtime_session.close()?;
    environment.stop_worker(&worker_id)?;
    let revoked = environment.request_backend_json(
        "DELETE",
        &format!("/api/w/{workspace}/external-workdir-grants/{grant_id}"),
        None,
    )?;
    assert_eq!(revoked["status"], "revoked");
    share.shutdown()?;
    drop(external);
    drop(runtime);
    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    assert!(!report.fixture_root_exists_after);
    let manager_path = manager_runtime.path().to_owned();
    manager_runtime.close()?;
    assert!(!manager_path.exists());
    Ok(())
}
