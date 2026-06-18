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
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempfile::TempDir;

const DEFAULT_WAIT: Duration = Duration::from_secs(5);
const DEFAULT_EXIT_WAIT: Duration = Duration::from_millis(1500);
static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub type Result<T> = std::result::Result<T, HarnessError>;

#[derive(Clone, Debug, Serialize)]
pub struct BinaryProviderInfo {
    pub provider: String,
    pub binary: PathBuf,
    pub workspace_root: PathBuf,
    pub cargo: Option<PathBuf>,
    pub build_args: Vec<String>,
    pub build_command: Option<String>,
    pub profile: String,
    pub tested_yoi_subprocess_env: TestedYoiEnvPolicy,
}

#[derive(Clone, Debug, Serialize)]
pub struct EnvPolicy {
    pub env_clear: bool,
    pub allowlist: Vec<String>,
    pub path_allowed: bool,
    pub provider_credentials_default_deny: Vec<String>,
    pub secret_patterns_default_deny: Vec<String>,
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TestedYoiEnvPolicy {
    pub fixture_setup: EnvPolicy,
    pub panel: EnvPolicy,
}

impl BinaryProviderInfo {
    fn log(&self) {
        match &self.build_command {
            Some(command) => eprintln!(
                "yoi-e2e binary provider={} command={} binary={}",
                self.provider,
                command,
                self.binary.display()
            ),
            None => eprintln!(
                "yoi-e2e binary provider={} binary={}",
                self.provider,
                self.binary.display()
            ),
        }
    }
}

fn env_policy(allowlist: &[&str], note: &str) -> EnvPolicy {
    EnvPolicy {
        env_clear: true,
        allowlist: allowlist.iter().map(|name| (*name).to_owned()).collect(),
        path_allowed: false,
        provider_credentials_default_deny: vec![
            "OPENAI_API_KEY".to_owned(),
            "ANTHROPIC_API_KEY".to_owned(),
            "GEMINI_API_KEY".to_owned(),
        ],
        secret_patterns_default_deny: vec![
            "*_API_KEY".to_owned(),
            "*_TOKEN".to_owned(),
            "*_SECRET".to_owned(),
            "*_CREDENTIAL*".to_owned(),
        ],
        note: note.to_owned(),
    }
}

fn fixture_setup_env_policy() -> EnvPolicy {
    env_policy(
        &[
            "HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CONFIG_HOME",
            "XDG_RUNTIME_DIR",
            "YOI_POD_RUNTIME_COMMAND",
        ],
        "tested yoi fixture setup commands use env_clear and receive only fixture HOME, XDG data/state/config/runtime dirs, and the explicit runtime binary override",
    )
}

fn tui_env_policy(include_hold_background_task: bool, include_rewind_fixture: bool) -> EnvPolicy {
    let mut allowlist = vec![
        "HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_RUNTIME_DIR",
        "TERM",
        "YOI_TUI_TEST_EVENTS",
        "YOI_POD_RUNTIME_COMMAND",
    ];
    if include_hold_background_task {
        allowlist.push("YOI_TUI_TEST_HOLD_BACKGROUND_TASK");
    }
    if include_rewind_fixture {
        allowlist.push("YOI_TUI_TEST_REWIND_FIXTURE");
    }
    env_policy(
        &allowlist,
        "tested yoi TUI subprocess uses env_clear and receives only fixture HOME, XDG data/state/config/runtime dirs, terminal/test-observer variables, explicit e2e fixture toggles, and the explicit runtime binary override",
    )
}

fn panel_env_policy(include_hold_background_task: bool) -> EnvPolicy {
    tui_env_policy(include_hold_background_task, false)
}

fn tested_yoi_env_policy_overview() -> TestedYoiEnvPolicy {
    TestedYoiEnvPolicy {
        fixture_setup: fixture_setup_env_policy(),
        panel: panel_env_policy(true),
    }
}

#[derive(Debug)]
pub enum HarnessError {
    Io(io::Error),
    Json(serde_json::Error),
    CommandFailed {
        program: PathBuf,
        args: Vec<String>,
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
    FullDragMouseCaptureEnabled {
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
                args,
                status,
                stdout,
                stderr,
            } => write!(
                f,
                "{} exited with {status}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                command_display(program, args)
            ),
            Self::Timeout { what, artifacts } => write!(
                f,
                "timed out waiting for {what}; artifacts at {}",
                artifacts.dir.display()
            ),
            Self::MissingBinary(path) => write!(
                f,
                "missing yoi binary {}; set YOI_E2E_BIN to an existing binary or inspect target/e2e-artifacts/binary-provider.json",
                path.display()
            ),
            Self::MouseCaptureNotEnabled { artifacts } => write!(
                f,
                "terminal mouse capture was not observed before mouse input; artifacts at {}",
                artifacts.dir.display()
            ),
            Self::FullDragMouseCaptureEnabled { artifacts } => write!(
                f,
                "forbidden full drag-motion mouse capture (?1002h/?1003h) was observed; artifacts at {}",
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
    pub xdg_runtime_dir: PathBuf,
    pub fixture_root: PathBuf,
    pub terminal_size: (u16, u16),
    pub hold_background_task: Option<String>,
    pub rewind_fixture: bool,
    pub command_args: Vec<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedPanelTicketRow {
    pub id: String,
    pub title: String,
    pub status: String,
}

impl ExpectedPanelTicketRow {
    pub fn new(id: impl Into<String>, title: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            status: status.into(),
        }
    }

    pub fn matches(&self, row: &RenderedPanelRow) -> bool {
        row.key.kind == "ticket"
            && row.key.id == self.id
            && row.title == self.title
            && row.status.as_deref() == Some(self.status.as_str())
    }

    fn description(&self) -> String {
        format!(
            "ticket row id={} title={:?} status={}",
            self.id, self.title, self.status
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowsRendered {
    pub selected: Option<PanelRowKey>,
    pub rows: Vec<RenderedPanelRow>,
}

impl RowsRendered {
    pub fn fixture_ticket_row(
        &self,
        expected: &ExpectedPanelTicketRow,
    ) -> Option<&RenderedPanelRow> {
        self.rows.iter().find(|row| expected.matches(row))
    }

    pub fn has_fixture_ticket_row(&self, expected: &ExpectedPanelTicketRow) -> bool {
        self.fixture_ticket_row(expected).is_some()
    }
}

fn rows_rendered_event_has_fixture_ticket(
    event: &HarnessEvent,
    expected: &ExpectedPanelTicketRow,
) -> bool {
    serde_json::from_value::<RowsRendered>(event.data.clone())
        .map(|rows| rows.has_fixture_ticket_row(expected))
        .unwrap_or(false)
}

fn describe_rows(rows: &RowsRendered) -> String {
    rows.rows
        .iter()
        .map(|row| {
            format!(
                "{}:{} title={:?} status={:?}",
                row.key.kind, row.key.id, row.title, row.status
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone)]
pub enum KeyPress {
    CtrlC,
    CtrlD,
    CtrlR,
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
        let env_policy =
            tui_env_policy(config.hold_background_task.is_some(), config.rewind_fixture);
        fs::write(
            &artifacts.run_json,
            serde_json::to_vec_pretty(&serde_json::json!({
                "binary": config.binary,
                "args": &config.command_args,
                "workspace": config.workspace,
                "home": config.home,
                "xdg_data_home": config.xdg_data_home,
                "xdg_state_home": config.xdg_state_home,
                "xdg_config_home": config.xdg_config_home,
                "xdg_runtime_dir": config.xdg_runtime_dir,
                "fixture_root": config.fixture_root,
                "runtime_policy": {
                    "host_runtime_inherited": false,
                    "host_xdg_runtime_dir_present": std::env::var_os("XDG_RUNTIME_DIR").is_some(),
                    "tested_yoi_runtime_source": "fixture XDG_RUNTIME_DIR"
                },
                "terminal_size": {
                    "columns": config.terminal_size.0,
                    "rows": config.terminal_size.1,
                },
                "hold_background_task": config.hold_background_task,
                "rewind_fixture": config.rewind_fixture,
                "tested_yoi_env_policy": &env_policy,
            }))?,
        )?;

        let (master, slave) = open_pty(config.terminal_size)?;
        let slave_for_stdin = slave.try_clone()?;
        let slave_for_stdout = slave.try_clone()?;

        let mut command = Command::new(&config.binary);
        command
            .args(&config.command_args)
            .env_clear()
            .env("YOI_TUI_TEST_EVENTS", &artifacts.events_jsonl)
            .env("YOI_POD_RUNTIME_COMMAND", &config.binary)
            .env("HOME", &config.home)
            .env("XDG_DATA_HOME", &config.xdg_data_home)
            .env("XDG_STATE_HOME", &config.xdg_state_home)
            .env("XDG_CONFIG_HOME", &config.xdg_config_home)
            .env("XDG_RUNTIME_DIR", &config.xdg_runtime_dir)
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(slave_for_stdin))
            .stdout(Stdio::from(slave_for_stdout))
            .stderr(Stdio::from(slave));
        if let Some(task) = &config.hold_background_task {
            command.env("YOI_TUI_TEST_HOLD_BACKGROUND_TASK", task);
        }
        if config.rewind_fixture {
            command.env("YOI_TUI_TEST_REWIND_FIXTURE", "1");
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

    /// Waits for the legacy `panel_ready` observer event, which means the first
    /// panel frame was painted. It intentionally does not mean workspace rows or
    /// Ticket data are ready.
    pub fn wait_for_first_visible_frame(&mut self, timeout: Duration) -> Result<HarnessEvent> {
        self.wait_for(
            "first visible panel frame (panel_ready, before rows readiness)",
            timeout,
            |event| event.event == "panel_ready",
        )
    }

    /// Waits until a concrete fixture Ticket row has rendered. This is the
    /// startup rows-ready signal; it validates id + title + state rather than
    /// using only the number of rendered rows.
    pub fn wait_for_fixture_ticket_rows_ready(
        &mut self,
        expected: &ExpectedPanelTicketRow,
        timeout: Duration,
    ) -> Result<RowsRendered> {
        let description = expected.description();
        let event = self.wait_for(
            format!("fixture Ticket rows ready ({description})"),
            timeout,
            |event| {
                event.event == "rows_rendered"
                    && rows_rendered_event_has_fixture_ticket(event, expected)
            },
        )?;
        serde_json::from_value(event.data).map_err(HarnessError::from)
    }

    pub fn assert_fixture_ticket_row_not_rendered(
        &mut self,
        expected: &ExpectedPanelTicketRow,
        duration: Duration,
    ) -> Result<()> {
        let start = Instant::now();
        while start.elapsed() < duration {
            if let Some(rows) = self
                .events()?
                .iter()
                .filter(|event| event.event == "rows_rendered")
                .filter_map(|event| serde_json::from_value::<RowsRendered>(event.data.clone()).ok())
                .find(|rows| rows.has_fixture_ticket_row(expected))
            {
                self.flush_output_artifact()?;
                return Err(HarnessError::Protocol(format!(
                    "fixture Ticket row rendered before data-backed rows readiness was expected: {}; rows: {}",
                    expected.description(),
                    describe_rows(&rows)
                )));
            }
            if let Some(status) = self.child.try_wait()? {
                self.flush_output_artifact()?;
                return Err(HarnessError::Protocol(format!(
                    "process exited with {status} while asserting fixture Ticket rows stayed delayed"
                )));
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
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

    pub fn assert_no_full_drag_mouse_capture(&mut self) -> Result<()> {
        if self.full_drag_mouse_capture_observed() {
            self.flush_output_artifact()?;
            return Err(HarnessError::FullDragMouseCaptureEnabled {
                artifacts: self.artifacts.clone(),
            });
        }
        Ok(())
    }

    pub fn expect_event(
        &mut self,
        event_name: &'static str,
        timeout: Duration,
    ) -> Result<HarnessEvent> {
        self.wait_for(event_name, timeout, |event| event.event == event_name)
    }

    pub fn count_events(&mut self, event_name: &str) -> Result<usize> {
        Ok(self
            .events()?
            .into_iter()
            .filter(|event| event.event == event_name)
            .count())
    }

    pub fn wait_for_no_additional_events(
        &mut self,
        event_name: &str,
        baseline: usize,
        duration: Duration,
    ) -> Result<()> {
        let start = Instant::now();
        while start.elapsed() < duration {
            if let Some(status) = self.child.try_wait()? {
                self.flush_output_artifact()?;
                return Err(HarnessError::Protocol(format!(
                    "process exited with {status} while waiting for no additional {event_name} events"
                )));
            }
            let count = self.count_events(event_name)?;
            if count > baseline {
                self.flush_output_artifact()?;
                return Err(HarnessError::Protocol(format!(
                    "observed {count} {event_name} events; expected no more than {baseline}"
                )));
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
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

    pub fn wheel_down(&mut self, row: &RenderedPanelRow) -> Result<()> {
        self.wheel(row, 65, "down")
    }

    pub fn wheel_up(&mut self, row: &RenderedPanelRow) -> Result<()> {
        self.wheel(row, 64, "up")
    }

    pub fn wheel(&mut self, row: &RenderedPanelRow, sgr_button: u8, label: &str) -> Result<()> {
        if !self.mouse_capture_enabled() {
            self.flush_output_artifact()?;
            return Err(HarnessError::MouseCaptureNotEnabled {
                artifacts: self.artifacts.clone(),
            });
        }
        let x = row.rect.x.saturating_add(1);
        let y = row.rect.y;
        self.write_input(
            &format!("mouse wheel {label} {} at {},{}", row.title, x, y),
            format!(
                "\u{1b}[<{};{};{}M",
                sgr_button,
                x.saturating_add(1),
                y.saturating_add(1)
            )
            .as_bytes(),
        )
    }

    pub fn press(&mut self, key: KeyPress) -> Result<()> {
        match key {
            KeyPress::CtrlC => self.write_input("Ctrl+C", b"\x03"),
            KeyPress::CtrlD => self.write_input("Ctrl+D", b"\x04"),
            KeyPress::CtrlR => self.write_input("Ctrl+R", b"\x12"),
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

    pub fn output_len(&self) -> usize {
        self.output.lock().map(|output| output.len()).unwrap_or(0)
    }

    pub fn wait_for_output_contains_from(
        &mut self,
        start_offset: usize,
        needle: &str,
        timeout: Duration,
    ) -> Result<()> {
        let start = Instant::now();
        let needle = needle.as_bytes();
        loop {
            if self.output_after(start_offset, needle) {
                return Ok(());
            }
            if let Some(status) = self.child.try_wait()? {
                self.flush_output_artifact()?;
                return Err(HarnessError::Protocol(format!(
                    "process exited with {status} before PTY output contained {:?}",
                    String::from_utf8_lossy(needle)
                )));
            }
            if start.elapsed() >= timeout {
                self.flush_output_artifact()?;
                return Err(HarnessError::Timeout {
                    what: format!(
                        "PTY output containing {:?} after offset {start_offset}",
                        String::from_utf8_lossy(needle)
                    ),
                    artifacts: self.artifacts.clone(),
                });
            }
            thread::sleep(Duration::from_millis(20));
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

    fn output_after(&self, start_offset: usize, needle: &[u8]) -> bool {
        if needle.is_empty() {
            return true;
        }
        self.output
            .lock()
            .map(|output| {
                let start = start_offset.min(output.len());
                output[start..]
                    .windows(needle.len())
                    .any(|window| window == needle)
            })
            .unwrap_or(false)
    }

    fn mouse_capture_enabled(&self) -> bool {
        self.output
            .lock()
            .map(|output| output_has_enabled_mouse_capture(&output))
            .unwrap_or(false)
    }

    fn full_drag_mouse_capture_observed(&self) -> bool {
        self.output
            .lock()
            .map(|output| output_has_full_drag_mouse_capture(&output))
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

#[derive(Debug, Clone, Serialize)]
pub struct FixtureCleanupReport {
    pub fixture_root: PathBuf,
    pub artifacts_dir: PathBuf,
    pub snapshot_dir: PathBuf,
    pub cleanup_attempted: bool,
    pub cleanup_success: bool,
    pub fixture_root_exists_after: bool,
    pub cleanup_error: Option<String>,
    pub report_path: PathBuf,
}

pub const READY_FIXTURE_TICKET_TITLE: &str = "Ready E2E Ticket";
pub const PLANNING_FIXTURE_TICKET_TITLE: &str = "Planning E2E Ticket";

#[derive(Debug)]
pub struct FixtureWorkspace {
    temp_root: Option<TempDir>,
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub xdg_state_home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_runtime_dir: PathBuf,
    pub artifacts_dir: PathBuf,
    pub ready_ticket_id: String,
    pub planning_ticket_id: String,
}

impl FixtureWorkspace {
    pub fn new(binary: &Path) -> Result<Self> {
        let workspace_root = workspace_root()?;
        let target_dir = workspace_root.join("target");
        let temp_parent = target_dir.join("e2e-tmp");
        let artifact_parent = target_dir.join("e2e-artifacts");
        fs::create_dir_all(&temp_parent)?;
        fs::create_dir_all(&artifact_parent)?;

        let fixture_id = format!(
            "{}-{}-{}",
            std::process::id(),
            now_ms(),
            FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let temp_root = tempfile::Builder::new()
            .prefix(&format!("yoi-e2e-{fixture_id}-"))
            .tempdir_in(&temp_parent)?;
        let root = temp_root.path().to_path_buf();
        let artifacts_dir = artifact_parent.join(fixture_id);
        let workspace = root.join("workspace");
        let home = root.join("home");
        let xdg_data_home = root.join("data");
        let xdg_state_home = root.join("state");
        let xdg_config_home = root.join("config");
        let xdg_runtime_dir = root.join("runtime");
        for dir in [
            &workspace,
            &home,
            &xdg_data_home,
            &xdg_state_home,
            &xdg_config_home,
            &xdg_runtime_dir,
            &artifacts_dir,
        ] {
            fs::create_dir_all(dir)?;
        }

        let mut fixture = Self {
            temp_root: Some(temp_root),
            root,
            workspace,
            home,
            xdg_data_home,
            xdg_state_home,
            xdg_config_home,
            xdg_runtime_dir,
            artifacts_dir,
            ready_ticket_id: String::new(),
            planning_ticket_id: String::new(),
        };
        fixture.write_fixture_metadata("created", None)?;

        write_blocking_pod_metadata(&fixture.xdg_data_home, "workspace")?;
        write_blocking_pod_metadata(&fixture.xdg_data_home, "workspace-orchestrator")?;
        run_yoi(
            binary,
            &fixture.workspace,
            &fixture.home,
            &fixture.xdg_data_home,
            &fixture.xdg_state_home,
            &fixture.xdg_config_home,
            &fixture.xdg_runtime_dir,
            &fixture.artifacts_dir,
            &["ticket", "init"],
        )?;
        let first = create_ticket(
            binary,
            &fixture.workspace,
            &fixture.home,
            &fixture.xdg_data_home,
            &fixture.xdg_state_home,
            &fixture.xdg_config_home,
            &fixture.xdg_runtime_dir,
            &fixture.artifacts_dir,
            READY_FIXTURE_TICKET_TITLE,
        )?;
        run_yoi(
            binary,
            &fixture.workspace,
            &fixture.home,
            &fixture.xdg_data_home,
            &fixture.xdg_state_home,
            &fixture.xdg_config_home,
            &fixture.xdg_runtime_dir,
            &fixture.artifacts_dir,
            &["ticket", "state", &first, "ready"],
        )?;
        let second = create_ticket(
            binary,
            &fixture.workspace,
            &fixture.home,
            &fixture.xdg_data_home,
            &fixture.xdg_state_home,
            &fixture.xdg_config_home,
            &fixture.xdg_runtime_dir,
            &fixture.artifacts_dir,
            PLANNING_FIXTURE_TICKET_TITLE,
        )?;
        fixture.ready_ticket_id = first;
        fixture.planning_ticket_id = second;
        fixture.write_fixture_metadata("ready", None)?;
        Ok(fixture)
    }

    pub fn ready_fixture_ticket_row(&self) -> ExpectedPanelTicketRow {
        ExpectedPanelTicketRow::new(
            self.ready_ticket_id.clone(),
            READY_FIXTURE_TICKET_TITLE,
            "ready",
        )
    }

    pub fn panel_config(&self, binary: PathBuf) -> PanelHarnessConfig {
        PanelHarnessConfig {
            binary,
            workspace: self.workspace.clone(),
            home: self.home.clone(),
            xdg_data_home: self.xdg_data_home.clone(),
            xdg_state_home: self.xdg_state_home.clone(),
            xdg_config_home: self.xdg_config_home.clone(),
            xdg_runtime_dir: self.xdg_runtime_dir.clone(),
            fixture_root: self.root.clone(),
            terminal_size: (100, 32),
            hold_background_task: None,
            rewind_fixture: false,
            command_args: vec![
                "panel".to_string(),
                "--workspace".to_string(),
                self.workspace.display().to_string(),
            ],
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

    pub fn rewind_fixture_config(&self, binary: PathBuf) -> PanelHarnessConfig {
        let mut config = self.panel_config(binary);
        config.rewind_fixture = true;
        config.command_args = vec![
            "--workspace".to_string(),
            self.workspace.display().to_string(),
            "--pod".to_string(),
            "e2e-rewind".to_string(),
        ];
        config.artifacts_dir = self.artifacts_dir.join("rewind");
        config
    }

    pub fn cleanup(mut self) -> Result<FixtureCleanupReport> {
        self.cleanup_inner(true)
    }

    fn cleanup_inner(&mut self, strict: bool) -> Result<FixtureCleanupReport> {
        let snapshot_dir = self.artifacts_dir.join("fixture-snapshot");
        if snapshot_dir.exists() {
            fs::remove_dir_all(&snapshot_dir)?;
        }
        copy_dir_recursive(&self.root, &snapshot_dir)?;

        let mut cleanup_error = None;
        if let Some(temp_root) = self.temp_root.take() {
            if let Err(err) = temp_root.close() {
                cleanup_error = Some(err.to_string());
            }
        }
        let fixture_root_exists_after = self.root.exists();
        let cleanup_success = cleanup_error.is_none() && !fixture_root_exists_after;
        let report = FixtureCleanupReport {
            fixture_root: self.root.clone(),
            artifacts_dir: self.artifacts_dir.clone(),
            snapshot_dir,
            cleanup_attempted: true,
            cleanup_success,
            fixture_root_exists_after,
            cleanup_error,
            report_path: self.artifacts_dir.join("cleanup.json"),
        };
        fs::write(&report.report_path, serde_json::to_vec_pretty(&report)?)?;
        self.write_fixture_metadata("cleaned", Some(&report))?;
        if strict && !report.cleanup_success {
            return Err(HarnessError::Protocol(format!(
                "fixture cleanup failed for {}; see {}",
                report.fixture_root.display(),
                report.report_path.display()
            )));
        }
        Ok(report)
    }

    fn write_fixture_metadata(
        &self,
        phase: &str,
        cleanup: Option<&FixtureCleanupReport>,
    ) -> Result<()> {
        fs::create_dir_all(&self.artifacts_dir)?;
        fs::write(
            self.artifacts_dir.join("fixture.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "phase": phase,
                "fixture_root": &self.root,
                "workspace": &self.workspace,
                "home": &self.home,
                "xdg_data_home": &self.xdg_data_home,
                "xdg_state_home": &self.xdg_state_home,
                "xdg_config_home": &self.xdg_config_home,
                "xdg_runtime_dir": &self.xdg_runtime_dir,
                "artifacts_dir": &self.artifacts_dir,
                "tickets": {
                    "ready": {
                        "id": &self.ready_ticket_id,
                        "title": READY_FIXTURE_TICKET_TITLE,
                        "state": "ready"
                    },
                    "planning": {
                        "id": &self.planning_ticket_id,
                        "title": PLANNING_FIXTURE_TICKET_TITLE,
                        "state": "planning"
                    }
                },
                "env_runtime_policy": {
                    "tested_yoi_uses_env_clear": true,
                    "host_runtime_inherited": false,
                    "host_xdg_runtime_dir_present": std::env::var_os("XDG_RUNTIME_DIR").is_some(),
                    "tested_yoi_runtime_source": "fixture XDG_RUNTIME_DIR",
                    "tested_yoi_pod_registry": self.xdg_runtime_dir.join("yoi").join("pods.json"),
                    "fixture_pod_metadata_root": self.xdg_data_home.join("yoi").join("pods")
                },
                "tested_yoi_env_policy": tested_yoi_env_policy_overview(),
                "cleanup": cleanup,
            }))?,
        )?;
        Ok(())
    }
}

impl Drop for FixtureWorkspace {
    fn drop(&mut self) {
        if self.temp_root.is_some() {
            let _ = self.cleanup_inner(false);
        }
    }
}

pub fn yoi_binary() -> Result<PathBuf> {
    Ok(yoi_binary_info()?.binary)
}

pub fn yoi_binary_info() -> Result<BinaryProviderInfo> {
    static BINARY_INFO: OnceLock<std::result::Result<BinaryProviderInfo, String>> = OnceLock::new();
    match BINARY_INFO.get_or_init(|| resolve_yoi_binary().map_err(|err| err.to_string())) {
        Ok(info) => Ok(info.clone()),
        Err(message) => Err(HarnessError::Protocol(message.clone())),
    }
}

fn resolve_yoi_binary() -> Result<BinaryProviderInfo> {
    if let Some(path) = std::env::var_os("YOI_E2E_BIN") {
        let workspace_root = workspace_root()?;
        let binary = PathBuf::from(path);
        let binary = if binary.is_absolute() {
            binary
        } else {
            workspace_root.join(binary)
        };
        if !binary.exists() {
            return Err(HarnessError::MissingBinary(binary));
        }
        let info = BinaryProviderInfo {
            provider: "YOI_E2E_BIN".to_owned(),
            binary,
            workspace_root,
            cargo: None,
            build_args: Vec::new(),
            build_command: None,
            profile: test_profile(),
            tested_yoi_subprocess_env: tested_yoi_env_policy_overview(),
        };
        info.log();
        write_binary_provider_artifact(&info)?;
        return Ok(info);
    }

    let workspace_root = workspace_root()?;
    let cargo = PathBuf::from(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    let mut args = vec![
        "build".to_owned(),
        "-p".to_owned(),
        "yoi".to_owned(),
        "--features".to_owned(),
        "e2e-test".to_owned(),
        "--bin".to_owned(),
        "yoi".to_owned(),
    ];
    if test_profile() == "release" {
        args.push("--release".to_owned());
    }

    let command = command_display(&cargo, &args);
    eprintln!("yoi-e2e binary provider=cargo-build command={command}");
    let output = Command::new(&cargo)
        .args(&args)
        .current_dir(&workspace_root)
        .output()?;
    if !output.status.success() {
        return Err(HarnessError::CommandFailed {
            program: cargo,
            args,
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let binary = current_target_profile_dir()?.join(binary_name());
    let info = BinaryProviderInfo {
        provider: "cargo-build".to_owned(),
        binary,
        workspace_root,
        cargo: Some(cargo),
        build_args: args,
        build_command: Some(command),
        profile: test_profile(),
        tested_yoi_subprocess_env: tested_yoi_env_policy_overview(),
    };
    info.log();
    write_binary_provider_artifact(&info)?;
    if !info.binary.exists() {
        return Err(HarnessError::MissingBinary(info.binary));
    }
    Ok(info)
}

fn workspace_root() -> Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| HarnessError::Protocol("could not resolve workspace root".to_owned()))
}

fn current_target_profile_dir() -> Result<PathBuf> {
    let mut path = std::env::current_exe()?;
    while let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        if name == "debug" || name == "release" {
            return Ok(path);
        }
        path.pop();
    }
    Ok(workspace_root()?.join("target").join(test_profile()))
}

fn test_profile() -> String {
    let Ok(mut path) = std::env::current_exe() else {
        return "debug".to_owned();
    };
    while let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        if name == "debug" || name == "release" {
            return name.to_owned();
        }
        path.pop();
    }
    "debug".to_owned()
}

fn binary_name() -> String {
    format!("yoi{}", std::env::consts::EXE_SUFFIX)
}

fn write_binary_provider_artifact(info: &BinaryProviderInfo) -> Result<()> {
    let dir = info.workspace_root.join("target").join("e2e-artifacts");
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("binary-provider.json"),
        serde_json::to_vec_pretty(info)?,
    )?;
    Ok(())
}

fn command_display(program: &Path, args: &[String]) -> String {
    std::iter::once(program.display().to_string())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ")
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
    runtime: &Path,
    artifacts_dir: &Path,
    title: &str,
) -> Result<String> {
    let output = run_yoi_capture(
        binary,
        workspace,
        home,
        data,
        state,
        config,
        runtime,
        artifacts_dir,
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
    runtime: &Path,
    artifacts_dir: &Path,
    args: &[&str],
) -> Result<()> {
    let output = run_yoi_capture(
        binary,
        workspace,
        home,
        data,
        state,
        config,
        runtime,
        artifacts_dir,
        args,
    )?;
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
    runtime: &Path,
    artifacts_dir: &Path,
    args: &[&str],
) -> Result<String> {
    let env_policy = fixture_setup_env_policy();
    append_fixture_command_artifact(artifacts_dir, workspace, binary, args, &env_policy)?;

    let mut command = Command::new(binary);
    command
        .args(args)
        .current_dir(workspace)
        .env_clear()
        .env("HOME", home)
        .env("XDG_DATA_HOME", data)
        .env("XDG_STATE_HOME", state)
        .env("XDG_CONFIG_HOME", config)
        .env("XDG_RUNTIME_DIR", runtime)
        .env("YOI_POD_RUNTIME_COMMAND", binary);

    let output = command.output()?;
    if !output.status.success() {
        return Err(HarnessError::CommandFailed {
            program: binary.to_path_buf(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(text)
}

fn append_fixture_command_artifact(
    artifacts_dir: &Path,
    workspace: &Path,
    binary: &Path,
    args: &[&str],
    env_policy: &EnvPolicy,
) -> Result<()> {
    fs::create_dir_all(artifacts_dir)?;
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(artifacts_dir.join("fixture-commands.jsonl"))?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({
            "ts_ms": now_ms(),
            "binary": binary,
            "workspace": workspace,
            "args": args,
            "tested_yoi_env_policy": env_policy,
        }),
    )?;
    writeln!(file)?;
    Ok(())
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), target)?;
        } else if file_type.is_symlink() {
            // Preserve enough diagnostics for E2E artifacts without following links out of
            // the fixture temp root.
            fs::write(
                target,
                format!("symlink -> {}\n", fs::read_link(entry.path())?.display()),
            )?;
        }
    }
    Ok(())
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

fn output_has_full_drag_mouse_capture(output: &[u8]) -> bool {
    output
        .windows(b"\x1b[?1002h".len())
        .any(|window| window == b"\x1b[?1002h")
        || output
            .windows(b"\x1b[?1003h".len())
            .any(|window| window == b"\x1b[?1003h")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_host_credentials_default_denied(policy: &EnvPolicy) {
        assert!(
            policy.env_clear,
            "tested yoi subprocesses must use env_clear"
        );
        assert!(
            !policy.path_allowed,
            "tested yoi subprocesses should not inherit or allow PATH"
        );
        assert!(
            !policy.allowlist.iter().any(|name| name == "PATH"),
            "PATH must not be allowlisted for tested yoi subprocesses"
        );
        for name in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY"] {
            assert!(
                !policy.allowlist.iter().any(|allowed| allowed == name),
                "{name} must not be allowlisted for tested yoi subprocesses"
            );
            assert!(
                policy
                    .provider_credentials_default_deny
                    .iter()
                    .any(|denied| denied == name),
                "{name} should be recorded as provider credential default-deny"
            );
        }
    }

    #[test]
    fn tested_yoi_env_policy_is_env_clear_allowlist() {
        let fixture = fixture_setup_env_policy();
        assert_host_credentials_default_denied(&fixture);
        assert_eq!(
            fixture.allowlist,
            [
                "HOME",
                "XDG_DATA_HOME",
                "XDG_STATE_HOME",
                "XDG_CONFIG_HOME",
                "XDG_RUNTIME_DIR",
                "YOI_POD_RUNTIME_COMMAND",
            ]
        );

        let panel = panel_env_policy(false);
        assert_host_credentials_default_denied(&panel);
        assert_eq!(
            panel.allowlist,
            [
                "HOME",
                "XDG_DATA_HOME",
                "XDG_STATE_HOME",
                "XDG_CONFIG_HOME",
                "XDG_RUNTIME_DIR",
                "TERM",
                "YOI_TUI_TEST_EVENTS",
                "YOI_POD_RUNTIME_COMMAND",
            ]
        );

        let panel_with_hold = panel_env_policy(true);
        assert_host_credentials_default_denied(&panel_with_hold);
        assert!(
            panel_with_hold
                .allowlist
                .iter()
                .any(|name| name == "YOI_TUI_TEST_HOLD_BACKGROUND_TASK")
        );
    }
}
