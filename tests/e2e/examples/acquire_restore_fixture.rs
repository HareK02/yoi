//! Explicit live-Codex acquisition, never invoked by a test or fixture loader.
//! Products must be featureless builds from the recorded clean source revision.
use protocol::{Event, SessionMessageRole, SessionSnapshot, SessionSnapshotEntryData, WorkerState};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    net::TcpStream,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, client::IntoClientRequest};
use worker_runtime::test_support::StoppedWorkerTemplate;
use yoi_e2e::{WorkerE2eEnvironment, worker_binary_provider_info};

#[path = "../tests/worker_restore/fixture.rs"]
#[allow(dead_code)]
mod fixture;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
const WAIT: Duration = Duration::from_secs(120);
const FIRST: &str = "これは隔離した復元テスト用の人工会話です。合言葉は『青い灯台』です。この合言葉を覚えて、一文だけで確認してください。Toolや外部操作は行わないでください。";
const SECOND: &str = "先ほど伝えた合言葉だけを答えてください。";

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let r = Command::new("git").args(args).current_dir(root).output()?;
    if !r.status.success() {
        return Err("Git provenance check failed".into());
    }
    Ok(String::from_utf8(r.stdout)?.trim().into())
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn write_json(path: &Path, value: &impl serde::Serialize) -> Result {
    let mut text = serde_json::to_vec_pretty(value)?;
    text.push(b'\n');
    fs::write(path, text)?;
    Ok(())
}
fn config(model: &str) -> Result<String> {
    let model = serde_json::to_string(model)?;
    Ok(format!(
        r#"{{
  profile = {{
    default_profile = "project:fixture-chat";
    entries = [{{
      selector = "project:fixture-chat";
      label = "Isolated Codex fixture conversation";
      description = "Artificial conversation only; no Workdir, Tools, Subject or background work.";
      profile = (import "$builtin/profiles/job.dcdl") // {{
        slug = "fixture-chat";
        worker = {{ mode = "tools"; }};
        model = {{ ref = {model}; }};
        engine = {{ instruction = "default"; reasoning = "low"; max_tokens = 2048; max_turns = 4; language = "Japanese"; }};
        session = {{ record_event_trace = false; }};
      }};
    }}];
  }};
  prompts = {{ default_prompt = "これは隔離環境の人工的な会話テストです。日本語で簡潔に応答してください。Toolや外部操作は行わず、ユーザーが会話中に提示した情報だけを使ってください。"; }};
}} as WorkspaceConfigSchema
"#
    ))
}
fn cli_config(env: &WorkerE2eEnvironment, binary: &Path, verb: &str, dir: &Path) -> Result {
    let (backend, workspace) = env.backend_target()?;
    let r = env.run_cli(
        binary,
        &[
            "--backend",
            backend,
            "--workspace-id",
            workspace,
            "config",
            verb,
            "--dir",
            dir.to_str().ok_or("non-UTF8 checkout")?,
        ],
    )?;
    if !r.status.success() {
        return Err(format!(
            "config {verb} failed: {}",
            String::from_utf8_lossy(&r.stderr)
        )
        .into());
    }
    Ok(())
}
fn connect(env: &WorkerE2eEnvironment, id: &str) -> Result<WebSocket<TcpStream>> {
    let authorized = env.worker_protocol_request(id)?;
    let mut url = authorized.url().clone();
    url.set_scheme("ws").map_err(|_| "invalid WS URL")?;
    let mut request = url.as_str().into_client_request()?;
    request.headers_mut().extend(authorized.headers().clone());
    let address = url
        .socket_addrs(|| None)?
        .into_iter()
        .find(|a| a.ip().is_loopback())
        .ok_or("not loopback")?;
    let stream = TcpStream::connect_timeout(&address, Duration::from_secs(10))?;
    stream.set_read_timeout(Some(WAIT))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    Ok(tungstenite::client(request, stream)?.0)
}
fn read_event(socket: &mut WebSocket<TcpStream>, deadline: Instant) -> Result<Event> {
    loop {
        socket.get_mut().set_read_timeout(Some(
            deadline
                .checked_duration_since(Instant::now())
                .ok_or("protocol timeout")?,
        ))?;
        match socket.read()? {
            Message::Text(s) => return Ok(serde_json::from_str(&s)?),
            Message::Ping(_) => socket.flush()?,
            Message::Pong(_) => (),
            _ => return Err("protocol closed during acquisition".into()),
        }
    }
}
fn snapshot(env: &WorkerE2eEnvironment, id: &str) -> Result<SessionSnapshot> {
    let mut socket = connect(env, id)?;
    match read_event(&mut socket, Instant::now() + WAIT)? {
        Event::Snapshot {
            session,
            state,
            in_flight,
            internal_workers,
            ..
        } if state.state == WorkerState::Idle
            && in_flight.is_empty()
            && internal_workers.is_empty() =>
        {
            Ok(session)
        }
        _ => Err("expected idle full snapshot".into()),
    }
}
fn turn(env: &WorkerE2eEnvironment, id: &str, input: &str) -> Result {
    let mut socket = connect(env, id)?;
    match read_event(&mut socket, Instant::now() + WAIT)? {
        Event::Snapshot { state, .. } if state.state == WorkerState::Idle => (),
        _ => return Err("Worker was not idle before input".into()),
    }
    let accepted = env.send_acquisition_input(id, input)?;
    if accepted["disposition"] != "accepted" {
        return Err("input was not accepted (not retried)".into());
    }
    let deadline = Instant::now() + WAIT;
    let mut running = false;
    loop {
        if let Event::WorkerState { snapshot } = read_event(&mut socket, deadline)? {
            if matches!(
                snapshot.state,
                WorkerState::Busy(protocol::WorkerBusyState::Run(
                    protocol::WorkerRunState::Running
                ))
            ) {
                running = true;
            }
            if running && snapshot.state == WorkerState::Idle {
                return Ok(());
            }
        }
    }
}
fn rows(snapshot: &SessionSnapshot) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    for entry in &snapshot.entries {
        let (role, parts) = match &entry.data {
            SessionSnapshotEntryData::UserInput { segments } => {
                ("user", serde_json::to_value(segments)?)
            }
            SessionSnapshotEntryData::Message {
                role: SessionMessageRole::Assistant,
                content,
            } => ("assistant", serde_json::to_value(content)?),
            SessionSnapshotEntryData::RunStats { .. } | SessionSnapshotEntryData::Invoke { .. } => {
                continue;
            }
            _ => return Err("unexpected non-conversation entry".into()),
        };
        rows.push(json!({"entry_id":entry.entry_id,"timestamp":entry.timestamp,"role":role,"parts":parts}));
    }
    Ok(rows)
}
fn acquire(codex_home: &Path, output: &Path, model: &str) -> Result {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    if output.exists() {
        return Err("output already exists; never overwrite captured fixtures".into());
    }
    if !model.starts_with("codex-oauth/") {
        return Err("this acquisition requires an explicit Codex OAuth model".into());
    }
    let revision = git(root, &["rev-parse", "HEAD"])?;
    if !git(root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err("commit tracked source before acquisition".into());
    }
    let configuration = config(model)?;
    let binaries = worker_binary_provider_info()?;
    let mut env = WorkerE2eEnvironment::new()?;
    env.use_acquisition_codex_home(codex_home)?;
    eprintln!("Acquisition artifacts: {}", env.artifacts_dir.display());
    env.start_backend()?;
    env.install_backend_token(&env.xdg_config_home)?;
    let checkout = env.root.join("config-acquisition");
    cli_config(&env, &binaries.binary("yoi")?, "pull", &checkout)?;
    fs::write(checkout.join("source/main.dcdl"), &configuration)?;
    cli_config(&env, &binaries.binary("yoi")?, "validate", &checkout)?;
    cli_config(&env, &binaries.binary("yoi")?, "push", &checkout)?;
    env.start_runtime_with_seed(|_| Ok(()))?;
    let created = env.create_acquisition_worker(fixture::PROFILE)?;
    // Public response only, never the authorized request or its headers.
    write_json(
        &env.artifacts_dir.join("acquisition-create-response.json"),
        &created,
    )?;
    let id = created
        .pointer("/worker/worker_id")
        .and_then(Value::as_str)
        .ok_or("Worker creation did not return a Worker (not retried)")?
        .to_owned();
    turn(&env, &id, FIRST)?;
    turn(&env, &id, SECOND)?;
    let live = snapshot(&env, &id)?;
    let live_rows = rows(&live)?;
    if live_rows.len() != 4
        || live_rows
            .iter()
            .map(|v| v["role"].as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            != ["user", "assistant", "user", "assistant"]
        || !live_rows[3]["parts"].to_string().contains("青い灯台")
    {
        return Err("two real conversation turns did not retain the artificial passphrase".into());
    }
    env.stop_worker(&id)?;
    env.shutdown()?;
    let source = env.runtime_data_dir.join("runtime/workers").join(&id);
    let template = StoppedWorkerTemplate::extract(&source)?;
    let template_value = serde_json::to_value(&template)?;
    let manifest_bytes: Vec<u8> =
        serde_json::from_value(template_value["session_manifest"].clone())?;
    let session_manifest: Value = serde_json::from_slice(&manifest_bytes)?;
    let session = session_store::WorkerSessionStore::open_read_only(source.join("session"))?;
    let (logs, _) = session.read_all_read_only_bounded(
        template.session_id(),
        template.segment_id(),
        8 * 1024 * 1024,
    )?;
    let projected = session_store::project_session_snapshot(template.session_id(), &logs);
    if rows(&projected)? != live_rows {
        return Err("offline projection differs from captured live conversation".into());
    }
    let source_files = [
        "worker.json".to_owned(),
        "metadata.json".into(),
        "session/session.json".into(),
        format!("session/segments/{}.jsonl", template.segment_id()),
    ];
    let mut source_hashes = BTreeMap::new();
    for file in source_files {
        source_hashes.insert(file.clone(), digest(&fs::read(source.join(file))?));
    }
    if git(root, &["rev-parse", "HEAD"])? != revision
        || !git(root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty()
    {
        return Err("source changed during acquisition".into());
    }
    fs::create_dir(output)?;
    write_json(&output.join("worker-template.json"), &template)?;
    fs::write(output.join("workspace-config.dcdl"), &configuration)?;
    write_json(
        &output.join("expected.json"),
        &json!({"source_revision":revision,"model":model,"worker_id":id,"session_id":template.session_id().to_string(),"rows":live_rows}),
    )?;
    let mut files = BTreeMap::new();
    for file in [
        "worker-template.json",
        "workspace-config.dcdl",
        "expected.json",
    ] {
        let bytes = fs::read(output.join(file))?;
        files.insert(file, json!({"bytes":bytes.len(),"sha256":digest(&bytes)}));
    }
    write_json(
        &output.join("manifest.json"),
        &json!({
            "format_version":1,"source_revision":revision,"runtime_schema_version":template_value["runtime_schema_version"],"session_schema_version":session_manifest["schema_version"],
            "workspace_id":template.workspace_id(),"runtime_id":template.runtime_id(),"worker_id":id,"session_id":template.session_id().to_string(),"segment_id":template.segment_id().to_string(),"model":model,"state":"stopped",
            "acquisition":"Real isolated Codex OAuth two-turn artificial conversation through featureless Server/Runtime public APIs; live snapshot compared with typed offline extraction. Session bytes unchanged; credentials and execution receipts excluded.",
            "source_file_sha256":source_hashes,"files":files
        }),
    )?;
    let captured = fixture::Fixture::load_from(output.to_path_buf())?;
    captured.verify_conversation(&projected)?;
    captured.source.assert_unchanged()?;
    for (file, expected) in source_hashes {
        if digest(&fs::read(source.join(file))?) != expected {
            return Err("source changed while publishing fixture".into());
        }
    }
    if !env.cleanup()?.cleanup_success {
        return Err("acquisition cleanup failed".into());
    }
    eprintln!("Captured and validated {}", output.display());
    Ok(())
}
fn main() -> Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 6 || args[0] != "--codex-home" || args[2] != "--output" || args[4] != "--model"
    {
        return Err("usage: acquire_restore_fixture --codex-home <authorized credential directory> --output <new fixture directory> --model <codex-oauth/model>".into());
    }
    acquire(&PathBuf::from(&args[1]), &PathBuf::from(&args[3]), &args[5])
}
