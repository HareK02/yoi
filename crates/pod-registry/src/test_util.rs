//! Shared test helpers for the pod-registry crate.
//!
//! Visible to all `#[cfg(test)]` modules under `crate::test_util::*`.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

use manifest::{Permission, ScopeRule};
use session_store::SegmentId;

use crate::table::LockFileGuard;

pub(crate) fn sid() -> SegmentId {
    session_store::new_segment_id()
}

/// Serialises tests that mutate runtime-dir env vars. The test
/// harness runs tests on multiple threads inside a single process,
/// so env-var writes from one test would otherwise leak into a
/// parallel test's `default_registry_path()` lookup.
pub(crate) static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Sandbox `INSOMNIA_RUNTIME_DIR` to a tempdir for the duration of
/// a test; restore the previous value (and any `INSOMNIA_HOME` /
/// `XDG_RUNTIME_DIR` that would otherwise outrank it) on drop.
pub(crate) struct RuntimeDirSandbox {
    prev_runtime: Option<String>,
    prev_home: Option<String>,
    prev_xdg: Option<String>,
    _guard: MutexGuard<'static, ()>,
}

impl RuntimeDirSandbox {
    pub(crate) fn new(dir: &Path) -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_runtime = std::env::var("INSOMNIA_RUNTIME_DIR").ok();
        let prev_home = std::env::var("INSOMNIA_HOME").ok();
        let prev_xdg = std::env::var("XDG_RUNTIME_DIR").ok();
        // SAFETY: ENV_LOCK serialises env writes across this test
        // module; other modules that touch env vars rely on their
        // own lock or `serial_test`.
        unsafe {
            std::env::remove_var("INSOMNIA_HOME");
            std::env::remove_var("XDG_RUNTIME_DIR");
            std::env::set_var("INSOMNIA_RUNTIME_DIR", dir);
        }
        Self {
            prev_runtime,
            prev_home,
            prev_xdg,
            _guard: guard,
        }
    }
}

impl Drop for RuntimeDirSandbox {
    fn drop(&mut self) {
        unsafe {
            match &self.prev_runtime {
                Some(v) => std::env::set_var("INSOMNIA_RUNTIME_DIR", v),
                None => std::env::remove_var("INSOMNIA_RUNTIME_DIR"),
            }
            match &self.prev_home {
                Some(v) => std::env::set_var("INSOMNIA_HOME", v),
                None => std::env::remove_var("INSOMNIA_HOME"),
            }
            match &self.prev_xdg {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }
}

pub(crate) fn write_rule(path: &str, recursive: bool) -> ScopeRule {
    ScopeRule {
        target: PathBuf::from(path),
        permission: Permission::Write,
        recursive,
    }
}

pub(crate) fn read_rule(path: &str, recursive: bool) -> ScopeRule {
    ScopeRule {
        target: PathBuf::from(path),
        permission: Permission::Read,
        recursive,
    }
}

pub(crate) fn sock(name: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/{name}.sock"))
}

pub(crate) fn open_empty(path: &Path) -> LockFileGuard {
    LockFileGuard::open(path).unwrap()
}
