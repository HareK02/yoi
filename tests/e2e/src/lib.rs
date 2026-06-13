//! Opt-in E2E helpers for driving the real `yoi panel` process through a PTY.
//!
//! The harness intentionally sends keyboard and mouse input only through the PTY.
//! Structured JSONL events emitted by the TUI are used for synchronization,
//! assertions, and failure artifacts; they are not an input or authority channel.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_WAIT: Duration = Duration::from_secs(5);
const DEFAULT_EXIT_WAIT: Duration = Duration::from_millis(1500);
static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub type Result<T> = std::result::Result<T, HarnessError>;

#[derive(Debug)]
pub enum HarnessError {
    Io(io::Error),
    Json(serde_json::Error),
    CommandFailed {
        program: PathBuf,
        status: ExitStatus,
        stdout: String,
        stderr: String,
    },
    Timeout {
        what: String,
        artifacts: PanelArtifacts,
    },
    MissingBinary(PathBuf),
    MouseCaptureNotEnabled {
        artifacts: PanelArtifacts,
    },
    Protocol(String),
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Json(err) => write!(f, "json error: {err}"),
            Self::CommandFailed {
                program,
                status,
                stdout,
                stderr,
            } => write!(
                f,
                "{} exited with {status}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                program.display()
            ),
            Self::Timeout { what, artifacts } => write!(
                f,
                "timed out waiting for {what}; artifacts at {}",
                artifacts.dir.display()
            ),
            Self::MissingBinary(path) => write!(
                f,
                "missing yoi binary {}; run `cargo build -p yoi --features e2e-test` or set YOI_E2E_BIN",
                path.display()
            ),
            Self::MouseCaptureNotEnabled { artifacts } => write!(
                f,
                "terminal mouse capture was not observed before mouse input; artifacts at {}",
                artifacts.dir.display()
            ),
            Self::Protocol(message) => write!(f, "protocol error: {message}"),
        }
    }
}

impl std::error::Error for HarnessError {}

impl From<io::Error> for HarnessError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for HarnessError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, Clone)]
pub struct PanelHarnessConfig {
    pub binary: PathBuf,
    pub workspace: PathBuf,
    pub home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub xdg_state_home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub terminal_size: (u16, u16),
    pub hold_background_task: Option<String>,
    pub artifacts_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessEvent {
    pub ts_ms: u128,
    pub surface: String,
    pub event: String,
    #[serde(default)]
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelRowKey {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanelRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedPanelRow {
    pub key: PanelRowKey,
    pub title: String,
    pub status: Option<String>,
    pub action: Option<String>,
    pub rect: PanelRect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowsRendered {
    pub selected: Option<PanelRowKey>,
    pub rows: Vec<RenderedPanelRow>,
}

#[derive(Debug, Clone)]
pub enum KeyPress {
    CtrlC,
    CtrlD,
    Enter,
    Esc,
    Text(String),
}

#[derive(Debug, Clone)]
pub struct PanelArtifacts {
    pub dir: PathBuf,
    pub events_jsonl: PathBuf,
    pub input_log: PathBuf,
    pub output_log: PathBuf,
    pub run_json: PathBuf,
}

pub struct PanelHarness {
    child: Child,
    master: File,
    reader: Option<JoinHandle<()>>,
    output: Arc<Mutex<Vec<u8>>>,
    last_event_offset: usize,
    artifacts: PanelArtifacts,
}

impl PanelHarness {
    pub fn spawn(config: PanelHarnessConfig) -> Result<Self> {
        if !config.binary.exists() {
            return Err(HarnessError::MissingBinary(config.binary));
        }
        fs::create_dir_all(&config.artifacts_dir)?;
        let artifacts = PanelArtifacts {
            dir: config.artifacts_dir.clone(),
            events_jsonl: config.artifacts_dir.join("events.jsonl"),
            input_log: config.artifacts_dir.join("input.log"),
            output_log: config.artifacts_dir.join("pty-output.log"),
            run_json: config.artifacts_dir.join("run.json"),
        };
        fs::write(&artifacts.events_jsonl, "")?;
        fs::write(&artifacts.input_log, "")?;
        fs::write(&artifacts.output_log, "")?;
        fs::write(
            &artifacts.run_json,
            serde_json::to_vec_pretty(&serde_json::json!({
                "binary": config.binary,
                "workspace": config.workspace,
                "home": config.home,
                "xdg_data_home": config.xdg_data_home,
                "xdg_state_home": config.xdg_state_home,
                "xdg_config_home": config.xdg_config_home,
                "terminal_size": {
                    "columns": config.terminal_size.0,
                    "rows": config.terminal_size.1,
                },
                "hold_background_task": config.hold_background_task,
            }))?,
        )?;

        let (master, slave) = open_pty(config.terminal_size)?;
        let slave_for_stdin = slave.try_clone()?;
        let slave_for_stdout = slave.try_clone()?;

        let mut command = Command::new(&config.binary);
        command
            .arg("panel")
            .arg("--workspace")
            .arg(&config.workspace)
            .env("YOI_TUI_TEST_EVENTS", &artifacts.events_jsonl)
            .env("YOI_POD_RUNTIME_COMMAND", &config.binary)
            .env("HOME", &config.home)
            .env("XDG_DATA_HOME", &config.xdg_data_home)
            .env("XDG_STATE_HOME", &config.xdg_state_home)
            .env("XDG_CONFIG_HOME", &config.xdg_config_home)
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(slave_for_stdin))
            .stdout(Stdio::from(slave_for_stdout))
            .stderr(Stdio::from(slave));
        if let Some(task) = &config.hold_background_task {
            command.env("YOI_TUI_TEST_HOLD_BACKGROUND_TASK", task);
        }
        let child = command.spawn()?;

        let output = Arc::new(Mutex::new(Vec::new()));
        let output_for_thread = Arc::clone(&output);
        let mut reader_file = master.try_clone()?;
        let output_log = artifacts.output_log.clone();
        let reader = thread::spawn(move || {
            let mut sink = OpenOptions::new()
                .append(true)
                .create(true)
                .open(output_log)
                .ok();
            let mut buf = [0_u8; 4096];
            loop {
                match reader_file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Some(sink) = sink.as_mut() {
                            let _ = sink.write_all(&buf[..n]);
                        }
                        if let Ok(mut output) = output_for_thread.lock() {
                            output.extend_from_slice(&buf[..n]);
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            child,
            master,
            reader: Some(reader),
            output,
            last_event_offset: 0,
            artifacts,
        })
    }

    pub fn wait_for<F>(
        &mut self,
        what: impl Into<String>,
        timeout: Duration,
        mut predicate: F,
    ) -> Result<HarnessEvent>
    where
        F: FnMut(&HarnessEvent) -> bool,
    {
        let what = what.into();
        let start = Instant::now();
        loop {
            for event in self.read_new_events()? {
                if predicate(&event) {
                    return Ok(event);
                }
            }
            if let Some(status) = self.child.try_wait()? {
                self.flush_output_artifact()?;
                return Err(HarnessError::Protocol(format!(
                    "process exited with {status} before {what}"
                )));
            }
            if start.elapsed() >= timeout {
                self.flush_output_artifact()?;
                return Err(HarnessError::Timeout {
                    what,
                    artifacts: self.artifacts.clone(),
                });
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn wait_for_rows(&mut self, min_rows: usize) -> Result<RowsRendered> {
        let event = self.wait_for("rows_rendered", DEFAULT_WAIT, |event| {
            event.event == "rows_rendered"
                && event
                    .data
                    .get("rows")
                    .and_then(Value::as_array)
                    .is_some_and(|rows| rows.len() >= min_rows)
        })?;
        serde_json::from_value(event.data).map_err(HarnessError::from)
    }

    pub fn expect_mouse_capture_enabled(&mut self) -> Result<()> {
        let start = Instant::now();
        loop {
            if self.mouse_capture_enabled() {
                return Ok(());
            }
            if start.elapsed() >= DEFAULT_WAIT {
                self.flush_output_artifact()?;
                return Err(HarnessError::MouseCaptureNotEnabled {
                    artifacts: self.artifacts.clone(),
                });
            }
            if let Some(status) = self.child.try_wait()? {
                self.flush_output_artifact()?;
                return Err(HarnessError::Protocol(format!(
                    "process exited with {status} before mouse capture was enabled"
                )));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn expect_background_task_pending(&mut self, task: &str) -> Result<()> {
        let start = Instant::now();
        loop {
            if background_task_is_pending(&self.events()?, task) {
                return Ok(());
            }
            if start.elapsed() >= DEFAULT_WAIT {
                self.flush_output_artifact()?;
                return Err(HarnessError::Timeout {
                    what: format!("background task {task:?} pending"),
                    artifacts: self.artifacts.clone(),
                });
            }
            if let Some(status) = self.child.try_wait()? {
                self.flush_output_artifact()?;
                return Err(HarnessError::Protocol(format!(
                    "process exited with {status} before background task {task:?} was pending"
                )));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn click(&mut self, row: &RenderedPanelRow) -> Result<()> {
        if !self.mouse_capture_enabled() {
            self.flush_output_artifact()?;
            return Err(HarnessError::MouseCaptureNotEnabled {
                artifacts: self.artifacts.clone(),
            });
        }
        let x = row.rect.x.saturating_add(1);
        let y = row.rect.y;
        self.write_input(
            &format!("mouse click {} at {},{}", row.title, x, y),
            format!("\u{1b}[<0;{};{}M", x.saturating_add(1), y.saturating_add(1)).as_bytes(),
        )
    }

    pub fn press(&mut self, key: KeyPress) -> Result<()> {
        match key {
            KeyPress::CtrlC => self.write_input("Ctrl+C", b"\x03"),
            KeyPress::CtrlD => self.write_input("Ctrl+D", b"\x04"),
            KeyPress::Enter => self.write_input("Enter", b"\r"),
            KeyPress::Esc => self.write_input("Esc", b"\x1b"),
            KeyPress::Text(text) => self.write_input(&format!("text {text:?}"), text.as_bytes()),
        }
    }

    pub fn expect_selection(&mut self, expected: &PanelRowKey) -> Result<HarnessEvent> {
        self.wait_for("selection_changed", DEFAULT_WAIT, |event| {
            event.event == "selection_changed"
                && event.data.get("selected").is_some_and(|selected| {
                    serde_json::from_value::<PanelRowKey>(selected.clone())
                        .is_ok_and(|actual| actual == *expected)
                })
        })
    }

    pub fn expect_exit_within(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                self.flush_output_artifact()?;
                let _ = self.reader.take();
                return Ok(status);
            }
            if start.elapsed() >= timeout {
                self.flush_output_artifact()?;
                return Err(HarnessError::Timeout {
                    what: format!("process exit within {timeout:?}"),
                    artifacts: self.artifacts.clone(),
                });
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn events(&mut self) -> Result<Vec<HarnessEvent>> {
        let text = fs::read_to_string(&self.artifacts.events_jsonl)?;
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).map_err(HarnessError::from))
            .collect()
    }

    pub fn artifacts(&self) -> &PanelArtifacts {
        &self.artifacts
    }

    pub fn default_exit_wait() -> Duration {
        DEFAULT_EXIT_WAIT
    }

    fn read_new_events(&mut self) -> Result<Vec<HarnessEvent>> {
        let text = fs::read_to_string(&self.artifacts.events_jsonl)?;
        let mut events = Vec::new();
        let new_text = text.get(self.last_event_offset..).unwrap_or_default();
        let mut consumed = self.last_event_offset;
        for segment in new_text.split_inclusive('\n') {
            if !segment.ends_with('\n') {
                break;
            }
            consumed += segment.len();
            let line = segment.trim();
            if !line.is_empty() {
                events.push(serde_json::from_str(line)?);
            }
        }
        self.last_event_offset = consumed;
        Ok(events)
    }

    fn write_input(&mut self, label: &str, bytes: &[u8]) -> Result<()> {
        let mut log = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.artifacts.input_log)?;
        writeln!(log, "{} {} bytes {label}", now_ms(), bytes.len())?;
        self.master.write_all(bytes)?;
        self.master.flush()?;
        Ok(())
    }

    fn mouse_capture_enabled(&self) -> bool {
        self.output
            .lock()
            .map(|output| output_has_enabled_mouse_capture(&output))
            .unwrap_or(false)
    }

    fn flush_output_artifact(&self) -> Result<()> {
        if let Ok(output) = self.output.lock() {
            fs::write(&self.artifacts.output_log, &*output)?;
        }
        Ok(())
    }
}

impl Drop for PanelHarness {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = self.flush_output_artifact();
        let _ = self.reader.take();
    }
}

#[derive(Debug)]
pub struct FixtureWorkspace {
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub xdg_state_home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub artifacts_dir: PathBuf,
}

impl FixtureWorkspace {
    pub fn new(binary: &Path) -> Result<Self> {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| {
                HarnessError::Protocol("could not resolve workspace root for artifacts".to_owned())
            })?
            .to_path_buf();
        let root = workspace_root
            .join("target")
            .join("e2e-artifacts")
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                now_ms(),
                FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
        let workspace = root.join("workspace");
        let home = root.join("home");
        let xdg_data_home = root.join("data");
        let xdg_state_home = root.join("state");
        let xdg_config_home = root.join("config");
        let artifacts_dir = root.join("artifacts");
        for dir in [
            &workspace,
            &home,
            &xdg_data_home,
            &xdg_state_home,
            &xdg_config_home,
            &artifacts_dir,
        ] {
            fs::create_dir_all(dir)?;
        }
        write_blocking_pod_metadata(&xdg_data_home, "workspace")?;
        write_blocking_pod_metadata(&xdg_data_home, "workspace-orchestrator")?;
        run_yoi(
            binary,
            &workspace,
            &home,
            &xdg_data_home,
            &xdg_state_home,
            &xdg_config_home,
            &["ticket", "init"],
        )?;
        let first = create_ticket(
            binary,
            &workspace,
            &home,
            &xdg_data_home,
            &xdg_state_home,
            &xdg_config_home,
            "Ready E2E Ticket",
        )?;
        run_yoi(
            binary,
            &workspace,
            &home,
            &xdg_data_home,
            &xdg_state_home,
            &xdg_config_home,
            &["ticket", "state", &first, "ready"],
        )?;
        let _second = create_ticket(
            binary,
            &workspace,
            &home,
            &xdg_data_home,
            &xdg_state_home,
            &xdg_config_home,
            "Planning E2E Ticket",
        )?;
        Ok(Self {
            root,
            workspace,
            home,
            xdg_data_home,
            xdg_state_home,
            xdg_config_home,
            artifacts_dir,
        })
    }

    pub fn panel_config(&self, binary: PathBuf) -> PanelHarnessConfig {
        PanelHarnessConfig {
            binary,
            workspace: self.workspace.clone(),
            home: self.home.clone(),
            xdg_data_home: self.xdg_data_home.clone(),
            xdg_state_home: self.xdg_state_home.clone(),
            xdg_config_home: self.xdg_config_home.clone(),
            terminal_size: (100, 32),
            hold_background_task: None,
            artifacts_dir: self.artifacts_dir.clone(),
        }
    }

    pub fn panel_config_holding_background_task(
        &self,
        binary: PathBuf,
        task: impl Into<String>,
    ) -> PanelHarnessConfig {
        let mut config = self.panel_config(binary);
        config.hold_background_task = Some(task.into());
        config
    }
}

pub fn yoi_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("YOI_E2E_BIN") {
        return PathBuf::from(path);
    }
    let mut path = std::env::current_exe().expect("current executable path");
    while let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        if name == "debug" || name == "release" {
            path.push("yoi");
            return path;
        }
        path.pop();
    }
    PathBuf::from("target/debug/yoi")
}

fn open_pty(size: (u16, u16)) -> Result<(File, File)> {
    let mut master = 0;
    let mut slave = 0;
    let mut winsize = libc::winsize {
        ws_row: size.1,
        ws_col: size.0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &mut winsize,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    let _ = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, 0) };
    Ok((master, slave))
}

fn create_ticket(
    binary: &Path,
    workspace: &Path,
    home: &Path,
    data: &Path,
    state: &Path,
    config: &Path,
    title: &str,
) -> Result<String> {
    let output = run_yoi_capture(
        binary,
        workspace,
        home,
        data,
        state,
        config,
        &["ticket", "create", "--title", title],
    )?;
    output
        .split_whitespace()
        .find(|part| part.len() >= 13 && part.chars().all(|ch| ch.is_ascii_alphanumeric()))
        .map(ToOwned::to_owned)
        .ok_or_else(|| HarnessError::Protocol(format!("could not parse ticket id from {output:?}")))
}

fn run_yoi(
    binary: &Path,
    workspace: &Path,
    home: &Path,
    data: &Path,
    state: &Path,
    config: &Path,
    args: &[&str],
) -> Result<()> {
    let output = run_yoi_capture(binary, workspace, home, data, state, config, args)?;
    drop(output);
    Ok(())
}

fn run_yoi_capture(
    binary: &Path,
    workspace: &Path,
    home: &Path,
    data: &Path,
    state: &Path,
    config: &Path,
    args: &[&str],
) -> Result<String> {
    let output = Command::new(binary)
        .args(args)
        .current_dir(workspace)
        .env("HOME", home)
        .env("XDG_DATA_HOME", data)
        .env("XDG_STATE_HOME", state)
        .env("XDG_CONFIG_HOME", config)
        .env("YOI_POD_RUNTIME_COMMAND", binary)
        .output()?;
    if !output.status.success() {
        return Err(HarnessError::CommandFailed {
            program: binary.to_path_buf(),
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(text)
}

fn write_blocking_pod_metadata(data_home: &Path, pod_name: &str) -> Result<()> {
    let dir = data_home.join("yoi").join("pods").join(pod_name);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("metadata.json"), b"not valid metadata for e2e\n")?;
    Ok(())
}

fn output_has_enabled_mouse_capture(output: &[u8]) -> bool {
    mouse_mode_enabled(output, b"\x1b[?1000h", b"\x1b[?1000l")
        && mouse_mode_enabled(output, b"\x1b[?1006h", b"\x1b[?1006l")
}

fn mouse_mode_enabled(output: &[u8], enable: &[u8], disable: &[u8]) -> bool {
    let last_enable = last_subsequence_index(output, enable);
    let last_disable = last_subsequence_index(output, disable);
    match (last_enable, last_disable) {
        (Some(enable), Some(disable)) => enable > disable,
        (Some(_), None) => true,
        _ => false,
    }
}

fn last_subsequence_index(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

fn background_task_is_pending(events: &[HarnessEvent], task: &str) -> bool {
    let mut pending = false;
    for event in events {
        if event.data.get("task").and_then(Value::as_str) != Some(task) {
            continue;
        }
        match event.event.as_str() {
            "background_task_started" => pending = true,
            "background_task_finished" | "background_task_aborted" => pending = false,
            _ => {}
        }
    }
    pending
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}
