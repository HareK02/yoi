use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

const EVENT_PATH_ENV: &str = "YOI_TUI_TEST_EVENTS";

static EVENT_WRITER: OnceLock<Option<Mutex<File>>> = OnceLock::new();

#[derive(Serialize)]
struct EventEnvelope<'a, T> {
    ts_ms: u128,
    surface: &'a str,
    event: &'a str,
    data: T,
}

pub(crate) fn emit<T>(surface: &'static str, event: &'static str, data: T)
where
    T: Serialize,
{
    let Some(writer) = EVENT_WRITER.get_or_init(open_event_writer).as_ref() else {
        return;
    };
    let Ok(mut writer) = writer.lock() else {
        return;
    };
    let envelope = EventEnvelope {
        ts_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default(),
        surface,
        event,
        data,
    };
    if serde_json::to_writer(&mut *writer, &envelope).is_ok() {
        let _ = writer.write_all(b"\n");
        let _ = writer.flush();
    }
}

fn open_event_writer() -> Option<Mutex<File>> {
    let path = std::env::var_os(EVENT_PATH_ENV).map(PathBuf::from)?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
        .map(Mutex::new)
}
