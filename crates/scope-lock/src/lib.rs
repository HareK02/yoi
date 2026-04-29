//! Machine-wide scope allocation registry.
//!
//! A single JSON file at `<runtime_dir>/scope.lock` records every live
//! Pod's scope allocation (see [`manifest::paths::scope_lock_path`] for
//! how the path is resolved). File-level `flock(2)` serialises access
//! across processes so spawn sequences from unrelated Pods can't race.
//!
//! Each Pod, when starting, acquires the lock, reclaims stale entries
//! (Pods whose PID has died), checks that its requested write scope
//! does not overlap any other allocation's effective write scope, and
//! registers itself. When it exits normally, it removes its entry and
//! returns delegated scope to its `delegated_from` parent. Crash
//! recovery rides on the next Pod that opens the file — no background
//! reaper.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;
use manifest::{Permission, ScopeRule, paths};
use serde::{Deserialize, Serialize};
use session_store::SessionId;

/// On-disk representation of the allocation table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockFile {
    #[serde(default)]
    pub allocations: Vec<Allocation>,
}

/// One Pod's scope allocation.
///
/// `scope_allow` is the full set of allow rules the Pod was granted.
/// Portions delegated out to child Pods are **not** subtracted in
/// storage — the effective write scope is derived on the fly by
/// removing rules owned by any Pod whose `delegated_from` points to
/// this one. Keeping the raw allow set makes reparenting (stale
/// reclaim) trivial.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Allocation {
    /// Pod name — also the identity used throughout orchestration.
    pub pod_name: String,
    /// Owning process. Checked with `kill(pid, 0)` for stale detection.
    pub pid: u32,
    /// Pod's Unix socket path.
    pub socket: PathBuf,
    /// Allow rules granted to this Pod (write + read).
    pub scope_allow: Vec<ScopeRule>,
    /// Name of the Pod that delegated scope to this one, or `None` for
    /// a top-level Pod started directly by a human.
    pub delegated_from: Option<String>,
    /// Session ID this Pod is currently writing to. `None` means this
    /// is a pre-reservation made by a spawner via [`delegate_scope`]
    /// before the child has come up; the child fills it in at
    /// [`adopt_allocation`] time.
    #[serde(default)]
    pub session_id: Option<SessionId>,
}

impl LockFile {
    pub fn find(&self, pod_name: &str) -> Option<&Allocation> {
        self.allocations.iter().find(|a| a.pod_name == pod_name)
    }

    pub fn find_mut(&mut self, pod_name: &str) -> Option<&mut Allocation> {
        self.allocations.iter_mut().find(|a| a.pod_name == pod_name)
    }

    /// Find the allocation currently writing to `session_id`. Skips
    /// pre-reservations whose `session_id` is still `None`.
    pub fn find_by_session(&self, session_id: SessionId) -> Option<&Allocation> {
        self.allocations
            .iter()
            .find(|a| a.session_id == Some(session_id))
    }
}

/// Default on-disk path: `<runtime_dir>/scope.lock` resolved via
/// [`manifest::paths::scope_lock_path`]. Tests should point this
/// elsewhere by setting `INSOMNIA_HOME` or `INSOMNIA_RUNTIME_DIR` to a
/// tempdir.
pub fn default_lock_path() -> io::Result<PathBuf> {
    paths::scope_lock_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve scope.lock path (no INSOMNIA_HOME / \
             INSOMNIA_RUNTIME_DIR / XDG_RUNTIME_DIR / HOME)",
        )
    })
}

/// RAII guard over an exclusively-locked lock file.
///
/// The file is kept open for the lifetime of the guard; `flock(LOCK_EX)`
/// is released automatically on drop. Mutations go through
/// [`LockFileGuard::data_mut`] and are committed with
/// [`LockFileGuard::save`] before dropping — callers who mutate but
/// never call `save` leave the table unchanged, which is the right
/// behaviour for error paths.
pub struct LockFileGuard {
    file: File,
    data: LockFile,
}

impl LockFileGuard {
    /// Open the lock file at `path` (creating it + parent dirs if
    /// needed), acquire an exclusive `flock`, then parse the contents.
    ///
    /// An empty file is treated as an empty allocation table.
    ///
    /// File is created with mode `0600` and its parent directory with
    /// mode `0700` so no other user on the machine can read the
    /// allocation table. Existing files/directories are left alone.
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        FileExt::lock_exclusive(&file)?;
        let mut this = Self {
            file,
            data: LockFile::default(),
        };
        this.reload()?;
        Ok(this)
    }

    fn reload(&mut self) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        let mut buf = String::new();
        self.file.read_to_string(&mut buf)?;
        self.data = if buf.trim().is_empty() {
            LockFile::default()
        } else {
            serde_json::from_str(&buf).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("scope.lock parse error: {e}"),
                )
            })?
        };
        Ok(())
    }

    pub fn data(&self) -> &LockFile {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut LockFile {
        &mut self.data
    }

    /// Serialise `self.data` back to the file (truncate + rewrite).
    pub fn save(&mut self) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(&self.data).map_err(io::Error::other)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.set_len(0)?;
        self.file.write_all(&json)?;
        self.file.sync_data()?;
        Ok(())
    }
}

impl Drop for LockFileGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Whether `a` and `b` claim any overlapping concrete path.
///
/// Recursive rules cover `target/**`; non-recursive rules cover the
/// target itself and its direct children. The four cases enumerate
/// when those coverage sets intersect.
pub(crate) fn rules_overlap(a: &ScopeRule, b: &ScopeRule) -> bool {
    match (a.recursive, b.recursive) {
        (true, true) => a.target.starts_with(&b.target) || b.target.starts_with(&a.target),
        (true, false) => {
            // a covers a.target/**; b covers {b.target, b.target/*}.
            b.target.starts_with(&a.target) || a.target.parent() == Some(b.target.as_path())
        }
        (false, true) => {
            a.target.starts_with(&b.target) || b.target.parent() == Some(a.target.as_path())
        }
        (false, false) => {
            a.target == b.target
                || a.target.parent() == Some(b.target.as_path())
                || b.target.parent() == Some(a.target.as_path())
        }
    }
}

/// Does `cover` fully contain `inner`'s claimed paths?
fn covers_fully(cover: &ScopeRule, inner: &ScopeRule) -> bool {
    if cover.permission < inner.permission {
        return false;
    }
    if cover.recursive {
        inner.target.starts_with(&cover.target)
    } else {
        inner.target == cover.target && !inner.recursive
    }
}

/// Check whether `rule` is contained in `parent`'s effective write
/// scope: its allow set covers `rule`, and no child of `parent` has
/// already taken a piece that would overlap `rule`.
pub fn is_within_effective_write(lock: &LockFile, parent: &str, rule: &ScopeRule) -> bool {
    let Some(alloc) = lock.find(parent) else {
        return false;
    };
    if rule.permission != Permission::Write {
        return alloc.scope_allow.iter().any(|r| covers_fully(r, rule));
    }
    let covered = alloc
        .scope_allow
        .iter()
        .filter(|r| r.permission == Permission::Write)
        .any(|r| covers_fully(r, rule));
    if !covered {
        return false;
    }
    let child_conflict = lock
        .allocations
        .iter()
        .filter(|a| a.delegated_from.as_deref() == Some(parent))
        .flat_map(|a| a.scope_allow.iter())
        .filter(|r| r.permission == Permission::Write)
        .any(|r| rules_overlap(r, rule));
    !child_conflict
}

/// Find the Pod that actually owns a write scope overlapping `rule`.
///
/// Walks the delegation tree: if an allocation overlaps `rule`, we
/// descend into its children and return the deepest overlapping node
/// as the true owner. `exempt` names a Pod whose ownership is
/// permitted (used during delegation: the spawner itself is allowed
/// to still own the rule's region because it is handing it down).
pub fn find_conflict_owner(
    lock: &LockFile,
    rule: &ScopeRule,
    exempt: Option<&str>,
) -> Option<String> {
    if rule.permission != Permission::Write {
        return None;
    }
    for alloc in lock
        .allocations
        .iter()
        .filter(|a| a.delegated_from.is_none())
    {
        if let Some(owner) = find_conflict_in_subtree(lock, alloc, rule) {
            if Some(owner.as_str()) == exempt {
                continue;
            }
            return Some(owner);
        }
    }
    None
}

fn find_conflict_in_subtree(
    lock: &LockFile,
    alloc: &Allocation,
    rule: &ScopeRule,
) -> Option<String> {
    let overlaps_here = alloc
        .scope_allow
        .iter()
        .filter(|r| r.permission == Permission::Write)
        .any(|r| rules_overlap(r, rule));
    if !overlaps_here {
        return None;
    }
    for child in lock
        .allocations
        .iter()
        .filter(|a| a.delegated_from.as_deref() == Some(alloc.pod_name.as_str()))
    {
        if let Some(owner) = find_conflict_in_subtree(lock, child, rule) {
            return Some(owner);
        }
    }
    Some(alloc.pod_name.clone())
}

// ---------------------------------------------------------------------------
// Mutating operations
// ---------------------------------------------------------------------------

/// Register a top-level Pod (started directly by a human, no
/// delegation parent). Reclaims stale entries before checking
/// conflicts so a crashed Pod's allocation doesn't block the new one.
///
/// Rejects when another live allocation is already writing to
/// `session_id`, so two `restore_from_manifest` calls under different
/// `pod_name`s cannot both grab the same session log.
pub fn register_pod(
    guard: &mut LockFileGuard,
    pod_name: String,
    pid: u32,
    socket: PathBuf,
    scope_allow: Vec<ScopeRule>,
    session_id: SessionId,
) -> Result<(), ScopeLockError> {
    reclaim_stale(guard);
    if guard.data().find(&pod_name).is_some() {
        return Err(ScopeLockError::DuplicatePodName(pod_name));
    }
    if let Some(existing) = guard.data().find_by_session(session_id) {
        return Err(ScopeLockError::SessionConflict {
            session_id,
            pod_name: existing.pod_name.clone(),
            socket: existing.socket.clone(),
        });
    }
    for rule in scope_allow
        .iter()
        .filter(|r| r.permission == Permission::Write)
    {
        if let Some(competitor) = find_conflict_owner(guard.data(), rule, None) {
            return Err(ScopeLockError::WriteConflict {
                competitor,
                rule: rule.clone(),
            });
        }
    }
    guard.data_mut().allocations.push(Allocation {
        pod_name,
        pid,
        socket,
        scope_allow,
        delegated_from: None,
        session_id: Some(session_id),
    });
    guard.save()?;
    Ok(())
}

/// Register a spawned Pod whose scope is delegated from `spawner`.
/// The requested scope must be within `spawner`'s effective write
/// scope; overlap with any Pod other than `spawner` is a conflict.
pub fn delegate_scope(
    guard: &mut LockFileGuard,
    spawner: &str,
    spawned: String,
    pid: u32,
    socket: PathBuf,
    scope_allow: Vec<ScopeRule>,
) -> Result<(), ScopeLockError> {
    reclaim_stale(guard);
    if guard.data().find(&spawned).is_some() {
        return Err(ScopeLockError::DuplicatePodName(spawned));
    }
    if guard.data().find(spawner).is_none() {
        return Err(ScopeLockError::UnknownPod(spawner.into()));
    }
    for rule in &scope_allow {
        if !is_within_effective_write(guard.data(), spawner, rule) {
            return Err(ScopeLockError::NotSubset {
                spawner: spawner.into(),
                rule: rule.clone(),
            });
        }
        if rule.permission == Permission::Write {
            if let Some(competitor) = find_conflict_owner(guard.data(), rule, Some(spawner)) {
                return Err(ScopeLockError::WriteConflict {
                    competitor,
                    rule: rule.clone(),
                });
            }
        }
    }
    guard.data_mut().allocations.push(Allocation {
        pod_name: spawned,
        pid,
        socket,
        scope_allow,
        delegated_from: Some(spawner.into()),
        // Pre-reservation. The child fills in its own session_id when
        // it calls `adopt_allocation` after the worker is built.
        session_id: None,
    });
    guard.save()?;
    Ok(())
}

/// Remove a Pod's allocation. Surviving children are reparented to
/// the removed Pod's own `delegated_from`, so the delegation tree
/// stays connected.
pub fn release_pod(guard: &mut LockFileGuard, pod_name: &str) -> Result<(), ScopeLockError> {
    let idx = guard
        .data()
        .allocations
        .iter()
        .position(|a| a.pod_name == pod_name);
    let Some(idx) = idx else {
        return Err(ScopeLockError::UnknownPod(pod_name.into()));
    };
    let removed = guard.data().allocations[idx].clone();
    for alloc in guard.data_mut().allocations.iter_mut() {
        if alloc.delegated_from.as_deref() == Some(pod_name) {
            alloc.delegated_from.clone_from(&removed.delegated_from);
        }
    }
    guard.data_mut().allocations.remove(idx);
    guard.save()?;
    Ok(())
}

/// Remove allocations whose PID is dead, reparenting children to the
/// dead Pod's `delegated_from`. Idempotent and best-effort — I/O
/// errors on save are swallowed so a crashed Pod's entry never blocks
/// forward progress.
pub fn reclaim_stale(guard: &mut LockFileGuard) {
    reclaim_stale_with(guard, pid_alive);
}

/// Test seam: stale reclaim with a caller-supplied liveness probe.
pub fn reclaim_stale_with(guard: &mut LockFileGuard, mut is_alive: impl FnMut(u32) -> bool) {
    let dead: Vec<String> = guard
        .data()
        .allocations
        .iter()
        .filter(|a| !is_alive(a.pid))
        .map(|a| a.pod_name.clone())
        .collect();
    if dead.is_empty() {
        return;
    }
    for name in &dead {
        let Some(idx) = guard
            .data()
            .allocations
            .iter()
            .position(|a| a.pod_name == *name)
        else {
            continue;
        };
        let removed = guard.data().allocations[idx].clone();
        for alloc in guard.data_mut().allocations.iter_mut() {
            if alloc.delegated_from.as_deref() == Some(name.as_str()) {
                alloc.delegated_from.clone_from(&removed.delegated_from);
            }
        }
        guard.data_mut().allocations.remove(idx);
    }
    let _ = guard.save();
}

/// `kill(pid, 0)` — returns true if the process exists (even when we
/// don't own it), false only on ESRCH.
fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if ret == 0 {
        return true;
    }
    io::Error::last_os_error()
        .raw_os_error()
        .map(|e| e != libc::ESRCH)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Lifecycle guard
// ---------------------------------------------------------------------------

/// Owned allocation: on drop, opens the lock file and releases this
/// Pod's entry. The guard keeps only the name + lock-file path; it
/// does not hold the `flock` for the Pod's lifetime.
#[derive(Debug)]
pub struct ScopeAllocationGuard {
    pod_name: String,
    lock_path: PathBuf,
}

impl ScopeAllocationGuard {
    pub fn pod_name(&self) -> &str {
        &self.pod_name
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

impl Drop for ScopeAllocationGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = LockFileGuard::open(&self.lock_path) {
            let _ = release_pod(&mut guard, &self.pod_name);
        }
    }
}

/// Open the default lock file, register a top-level Pod, and return a
/// guard that will release the allocation on drop.
pub fn install_top_level(
    pod_name: String,
    pid: u32,
    socket: PathBuf,
    scope_allow: Vec<ScopeRule>,
    session_id: SessionId,
) -> Result<ScopeAllocationGuard, ScopeLockError> {
    let lock_path = default_lock_path()?;
    let mut guard = LockFileGuard::open(&lock_path)?;
    register_pod(
        &mut guard,
        pod_name.clone(),
        pid,
        socket,
        scope_allow,
        session_id,
    )?;
    Ok(ScopeAllocationGuard {
        pod_name,
        lock_path,
    })
}

/// Take ownership of an existing allocation that was pre-registered by
/// a spawning Pod.
///
/// The spawning flow is two-stage: the spawner calls [`delegate_scope`]
/// (with its own pid as a live placeholder, `session_id = None`), then
/// exec's the child; the child, once running, calls this function to
/// rewrite the allocation's pid + session_id to its own and claim the
/// `ScopeAllocationGuard` so the entry is released when the child
/// exits.
pub fn adopt_allocation(
    pod_name: String,
    new_pid: u32,
    session_id: SessionId,
) -> Result<ScopeAllocationGuard, ScopeLockError> {
    let lock_path = default_lock_path()?;
    let mut guard = LockFileGuard::open(&lock_path)?;
    let alloc = guard
        .data_mut()
        .find_mut(&pod_name)
        .ok_or_else(|| ScopeLockError::UnknownPod(pod_name.clone()))?;
    alloc.pid = new_pid;
    alloc.session_id = Some(session_id);
    guard.save()?;
    Ok(ScopeAllocationGuard {
        pod_name,
        lock_path,
    })
}

/// Rewrite the `session_id` recorded for `pod_name` to
/// `new_session_id`.
///
/// The Pod's in-memory `session_id` can change underneath the
/// allocation in two normal places:
///
/// - `Pod::compact` mints a fresh session and swaps it in.
/// - `session_store::ensure_head_or_fork` auto-forks when another
///   writer has advanced the store head behind our back.
///
/// Both paths must call this so subsequent `lookup_session` queries
/// find the live session id, not the old one. Without this update a
/// concurrent `restore_from_manifest(new_id)` would see "no live
/// writer" and proceed to register a competing allocation on the
/// session this Pod just moved into.
///
/// The lock is opened once and the allocation is rewritten inside the
/// guard, so the session_id collision check is atomic with the
/// rewrite.
pub fn update_session(pod_name: &str, new_session_id: SessionId) -> Result<(), ScopeLockError> {
    let lock_path = default_lock_path()?;
    let mut guard = LockFileGuard::open(&lock_path)?;
    if let Some(other) = guard.data().find_by_session(new_session_id) {
        if other.pod_name != pod_name {
            return Err(ScopeLockError::SessionConflict {
                session_id: new_session_id,
                pod_name: other.pod_name.clone(),
                socket: other.socket.clone(),
            });
        }
    }
    let alloc = guard
        .data_mut()
        .find_mut(pod_name)
        .ok_or_else(|| ScopeLockError::UnknownPod(pod_name.into()))?;
    alloc.session_id = Some(new_session_id);
    guard.save()?;
    Ok(())
}

/// Information about a Pod that currently holds an allocation for a
/// given session.
#[derive(Debug, Clone)]
pub struct SessionLockInfo {
    pub pod_name: String,
    pub socket: PathBuf,
    pub pid: u32,
}

/// Open the default lock file, reclaim stale entries, and return the
/// allocation currently writing to `session_id`, if any.
///
/// Used by `Pod::restore_from_manifest` to refuse a resume that would
/// race a live writer on the same source session.
pub fn lookup_session(session_id: SessionId) -> Result<Option<SessionLockInfo>, ScopeLockError> {
    let lock_path = default_lock_path()?;
    let mut guard = LockFileGuard::open(&lock_path)?;
    reclaim_stale(&mut guard);
    Ok(guard.data().find_by_session(session_id).map(|a| {
        SessionLockInfo {
            pod_name: a.pod_name.clone(),
            socket: a.socket.clone(),
            pid: a.pid,
        }
    }))
}

/// Errors raised by the mutating scope-lock operations.
#[derive(Debug, thiserror::Error)]
pub enum ScopeLockError {
    #[error("I/O error on scope.lock: {0}")]
    Io(#[from] io::Error),
    #[error("pod name `{0}` is already registered")]
    DuplicatePodName(String),
    #[error("requested scope `{}` conflicts with pod `{competitor}`", .rule.target.display())]
    WriteConflict { competitor: String, rule: ScopeRule },
    #[error(
        "requested scope `{}` is not within spawner `{spawner}`'s effective scope",
        .rule.target.display()
    )]
    NotSubset { spawner: String, rule: ScopeRule },
    #[error("pod `{0}` is not registered")]
    UnknownPod(String),
    #[error(
        "session {session_id} is already held by pod `{pod_name}` at {}",
        .socket.display()
    )]
    SessionConflict {
        session_id: SessionId,
        pod_name: String,
        socket: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::Permission;
    use std::sync::{LazyLock, Mutex, MutexGuard};
    use tempfile::TempDir;

    /// Serialises tests that mutate runtime-dir env vars. The test
    /// harness runs tests on multiple threads inside a single process,
    /// so env-var writes from one test would otherwise leak into a
    /// parallel test's `default_lock_path()` lookup.
    fn sid() -> SessionId {
        session_store::new_session_id()
    }

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    /// Sandbox `INSOMNIA_RUNTIME_DIR` to a tempdir for the duration of
    /// a test; restore the previous value (and any `INSOMNIA_HOME` /
    /// `XDG_RUNTIME_DIR` that would otherwise outrank it) on drop.
    struct RuntimeDirSandbox {
        prev_runtime: Option<String>,
        prev_home: Option<String>,
        prev_xdg: Option<String>,
        _guard: MutexGuard<'static, ()>,
    }

    impl RuntimeDirSandbox {
        fn new(dir: &Path) -> Self {
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

    fn write_rule(path: &str, recursive: bool) -> ScopeRule {
        ScopeRule {
            target: PathBuf::from(path),
            permission: Permission::Write,
            recursive,
        }
    }

    fn sock(name: &str) -> PathBuf {
        PathBuf::from(format!("/tmp/{name}.sock"))
    }

    fn open_empty(path: &Path) -> LockFileGuard {
        LockFileGuard::open(path).unwrap()
    }

    #[test]
    fn open_creates_empty_lock_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let guard = LockFileGuard::open(&path).unwrap();
        assert!(guard.data().allocations.is_empty());
        assert!(path.exists());
    }

    #[test]
    fn open_creates_file_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("insomnia");
        let path = parent.join("scope.lock");
        let _guard = LockFileGuard::open(&path).unwrap();
        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "file mode = {file_mode:o}");
        let dir_mode = std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "dir mode = {dir_mode:o}");
    }

    #[test]
    fn save_and_reopen_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        {
            let mut g = open_empty(&path);
            register_pod(
                &mut g,
                "a".into(),
                std::process::id(),
                sock("a"),
                vec![write_rule("/src", true)],
                sid(),
            )
            .unwrap();
        }
        let guard = LockFileGuard::open(&path).unwrap();
        assert_eq!(guard.data().allocations.len(), 1);
        assert_eq!(guard.data().allocations[0].pod_name, "a");
    }

    #[test]
    fn rules_overlap_prefix_relation() {
        assert!(rules_overlap(
            &write_rule("/src", true),
            &write_rule("/src/core", true)
        ));
        assert!(rules_overlap(
            &write_rule("/src/core", true),
            &write_rule("/src", true),
        ));
        assert!(!rules_overlap(
            &write_rule("/src", true),
            &write_rule("/docs", true),
        ));
    }

    #[test]
    fn rules_overlap_non_recursive() {
        assert!(!rules_overlap(
            &write_rule("/src", false),
            &write_rule("/src/a/b", true),
        ));
        assert!(rules_overlap(
            &write_rule("/src", false),
            &write_rule("/src/child", false),
        ));
    }

    #[test]
    fn register_detects_write_conflict() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        let err = register_pod(
            &mut g,
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![write_rule("/src/core", true)],
            sid(),
        )
        .unwrap_err();
        match err {
            ScopeLockError::WriteConflict { competitor, .. } => assert_eq!(competitor, "a"),
            other => panic!("expected WriteConflict, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_pod_name_rejected() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        let err = register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a2"),
            vec![write_rule("/docs", true)],
            sid(),
        )
        .unwrap_err();
        assert!(matches!(err, ScopeLockError::DuplicatePodName(ref n) if n == "a"));
    }

    #[test]
    fn delegate_must_be_subset() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        let err = delegate_scope(
            &mut g,
            "a",
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![write_rule("/docs", true)],
        )
        .unwrap_err();
        assert!(matches!(err, ScopeLockError::NotSubset { .. }));
    }

    #[test]
    fn delegate_succeeds_within_parent_scope() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        delegate_scope(
            &mut g,
            "a",
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![write_rule("/src/core", true)],
        )
        .unwrap();
        assert_eq!(g.data().allocations.len(), 2);
        // A's effective write no longer covers /src/core because B has it.
        assert!(!is_within_effective_write(
            g.data(),
            "a",
            &write_rule("/src/core", true)
        ));
        // A still covers its own uninvolved areas.
        assert!(is_within_effective_write(
            g.data(),
            "a",
            &write_rule("/src/other", true)
        ));
    }

    #[test]
    fn delegate_rejects_sibling_overlap() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        delegate_scope(
            &mut g,
            "a",
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![write_rule("/src/core", true)],
        )
        .unwrap();
        // Sibling C from A tries to take /src/core/sub — already under B's scope.
        let err = delegate_scope(
            &mut g,
            "a",
            "c".into(),
            std::process::id(),
            sock("c"),
            vec![write_rule("/src/core/sub", true)],
        )
        .unwrap_err();
        // NotSubset fires first because /src/core is no longer in A's effective.
        assert!(matches!(err, ScopeLockError::NotSubset { .. }));
    }

    #[test]
    fn release_reparents_children() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        delegate_scope(
            &mut g,
            "a",
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![write_rule("/src/core", true)],
        )
        .unwrap();
        delegate_scope(
            &mut g,
            "b",
            "d".into(),
            std::process::id(),
            sock("d"),
            vec![write_rule("/src/core/x", true)],
        )
        .unwrap();
        release_pod(&mut g, "b").unwrap();
        // D should now list A as its delegated_from.
        let d = g.data().find("d").unwrap();
        assert_eq!(d.delegated_from.as_deref(), Some("a"));
        assert!(g.data().find("b").is_none());
    }

    #[test]
    fn reclaim_stale_reparents_and_removes_dead_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        delegate_scope(
            &mut g,
            "a",
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![write_rule("/src/core", true)],
        )
        .unwrap();
        delegate_scope(
            &mut g,
            "b",
            "d".into(),
            std::process::id(),
            sock("d"),
            vec![write_rule("/src/core/x", true)],
        )
        .unwrap();
        // Simulate B crashing by rewriting its pid to one the probe
        // will treat as dead.
        let fake_dead_pid: u32 = 0xffff_fff0;
        for alloc in g.data_mut().allocations.iter_mut() {
            if alloc.pod_name == "b" {
                alloc.pid = fake_dead_pid;
            }
        }
        reclaim_stale_with(&mut g, |pid| pid != fake_dead_pid);
        assert!(g.data().find("b").is_none());
        let d = g.data().find("d").unwrap();
        assert_eq!(d.delegated_from.as_deref(), Some("a"));
    }

    fn read_rule(path: &str, recursive: bool) -> ScopeRule {
        ScopeRule {
            target: PathBuf::from(path),
            permission: Permission::Read,
            recursive,
        }
    }

    #[test]
    fn read_rules_do_not_conflict_with_write() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        // B only reads under the same tree — allowed.
        register_pod(
            &mut g,
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![read_rule("/src", true)],
            sid(),
        )
        .unwrap();
        assert_eq!(g.data().allocations.len(), 2);
    }

    #[test]
    fn releasing_pod_reopens_scope_for_fresh_registration() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        release_pod(&mut g, "a").unwrap();
        register_pod(
            &mut g,
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
    }

    #[test]
    fn delegated_scope_returns_to_parent_on_release() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        delegate_scope(
            &mut g,
            "a",
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![write_rule("/src/core", true)],
        )
        .unwrap();
        assert!(!is_within_effective_write(
            g.data(),
            "a",
            &write_rule("/src/core", true)
        ));
        release_pod(&mut g, "b").unwrap();
        // /src/core is back in A's effective write scope.
        assert!(is_within_effective_write(
            g.data(),
            "a",
            &write_rule("/src/core", true)
        ));
    }

    #[test]
    fn scope_allocation_guard_releases_on_drop() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let lock_path = dir.path().join("scope.lock");
        let guard = install_top_level(
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        {
            let g = LockFileGuard::open(&lock_path).unwrap();
            assert!(g.data().find("a").is_some());
        }
        drop(guard);
        {
            let g = LockFileGuard::open(&lock_path).unwrap();
            assert!(g.data().find("a").is_none());
        }
    }

    #[test]
    fn adopt_allocation_rewrites_pid_and_releases_on_drop() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let lock_path = dir.path().join("scope.lock");
        // Pre-register an allocation under spawner's pid, as delegate_scope would.
        {
            let mut g = LockFileGuard::open(&lock_path).unwrap();
            delegate_placeholder(&mut g, "child", std::process::id());
        }
        let child_pid = std::process::id().wrapping_add(1);
        let guard = adopt_allocation("child".into(), child_pid, sid()).unwrap();
        {
            let g = LockFileGuard::open(&lock_path).unwrap();
            let alloc = g.data().find("child").unwrap();
            assert_eq!(alloc.pid, child_pid);
        }
        drop(guard);
        {
            let g = LockFileGuard::open(&lock_path).unwrap();
            assert!(g.data().find("child").is_none());
        }
    }

    #[test]
    fn adopt_allocation_errors_on_unknown_pod() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let err = adopt_allocation("ghost".into(), 42, sid()).unwrap_err();
        assert!(matches!(err, ScopeLockError::UnknownPod(ref n) if n == "ghost"));
    }

    /// Mimic what the spawner does before the child comes up: push an
    /// allocation for the child carrying the spawner's (live) pid as a
    /// placeholder. Exists only in tests.
    fn delegate_placeholder(g: &mut LockFileGuard, pod_name: &str, placeholder_pid: u32) {
        g.data_mut().allocations.push(Allocation {
            pod_name: pod_name.to_string(),
            pid: placeholder_pid,
            socket: sock(pod_name),
            scope_allow: vec![write_rule("/tmp/child", true)],
            delegated_from: None,
            session_id: None,
        });
        g.save().unwrap();
    }

    #[test]
    fn conflict_detection_descends_to_real_owner() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        register_pod(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        delegate_scope(
            &mut g,
            "a",
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![write_rule("/src/core", true)],
        )
        .unwrap();
        // A different top-level Pod trying to register /src/core/x
        // should be blamed on B (deepest owner), not A.
        let err = register_pod(
            &mut g,
            "x".into(),
            std::process::id(),
            sock("x"),
            vec![write_rule("/src/core/x", true)],
            sid(),
        )
        .unwrap_err();
        match err {
            ScopeLockError::WriteConflict { competitor, .. } => assert_eq!(competitor, "b"),
            other => panic!("expected WriteConflict, got {other:?}"),
        }
    }

    #[test]
    fn find_by_session_skips_none_placeholders() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        // Pre-reservation: delegate_scope leaves session_id = None
        // until adopt_allocation rewrites it. find_by_session must not
        // match those placeholders, otherwise a freshly-spawning child
        // would shadow itself before it has even chosen a session.
        register_pod(
            &mut g,
            "parent".into(),
            std::process::id(),
            sock("parent"),
            vec![write_rule("/p", true)],
            sid(),
        )
        .unwrap();
        delegate_scope(
            &mut g,
            "parent",
            "child".into(),
            std::process::id(),
            sock("child"),
            vec![write_rule("/p/sub", true)],
        )
        .unwrap();

        let target_session = sid();
        // The placeholder allocation has session_id = None and must
        // not be returned for any lookup.
        assert!(g.data().find_by_session(target_session).is_none());

        // After adopt-style rewrite, the same allocation is now found.
        g.data_mut()
            .find_mut("child")
            .unwrap()
            .session_id = Some(target_session);
        let found = g.data().find_by_session(target_session).unwrap();
        assert_eq!(found.pod_name, "child");
    }

    #[test]
    fn register_pod_rejects_session_id_collision() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scope.lock");
        let mut g = open_empty(&path);
        let shared_session = sid();
        register_pod(
            &mut g,
            "first".into(),
            std::process::id(),
            sock("first"),
            vec![write_rule("/work/a", true)],
            shared_session,
        )
        .unwrap();
        // Second registration tries to grab the same session_id under
        // a different pod_name. Without the SessionConflict check both
        // would succeed and race on the same jsonl.
        let err = register_pod(
            &mut g,
            "second".into(),
            std::process::id(),
            sock("second"),
            vec![write_rule("/work/b", true)],
            shared_session,
        )
        .unwrap_err();
        match err {
            ScopeLockError::SessionConflict {
                session_id,
                pod_name,
                ..
            } => {
                assert_eq!(session_id, shared_session);
                assert_eq!(pod_name, "first");
            }
            other => panic!("expected SessionConflict, got {other:?}"),
        }
    }

    #[test]
    fn lookup_session_returns_live_writer_info() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let s = sid();
        let guard = install_top_level(
            "live".into(),
            std::process::id(),
            sock("live"),
            vec![write_rule("/work", true)],
            s,
        )
        .unwrap();
        let info = lookup_session(s).unwrap().expect("expected live writer");
        assert_eq!(info.pod_name, "live");
        assert_eq!(info.socket, sock("live"));
        drop(guard);
        // After the guard's release, the lookup goes back to None.
        assert!(lookup_session(s).unwrap().is_none());
    }

    #[test]
    fn update_session_rewrites_allocation_session_id() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let original = sid();
        let updated = sid();
        let _guard = install_top_level(
            "p".into(),
            std::process::id(),
            sock("p"),
            vec![write_rule("/work", true)],
            original,
        )
        .unwrap();
        update_session("p", updated).unwrap();
        // lookup against the original is now empty, the updated id wins.
        assert!(lookup_session(original).unwrap().is_none());
        assert_eq!(lookup_session(updated).unwrap().unwrap().pod_name, "p");
    }

    #[test]
    fn update_session_rejects_when_target_already_held() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let s_a = sid();
        let s_b = sid();
        let _g_a = install_top_level(
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/work/a", true)],
            s_a,
        )
        .unwrap();
        let _g_b = install_top_level(
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![write_rule("/work/b", true)],
            s_b,
        )
        .unwrap();
        // `a` cannot adopt b's live session id.
        let err = update_session("a", s_b).unwrap_err();
        match err {
            ScopeLockError::SessionConflict {
                pod_name,
                session_id,
                ..
            } => {
                assert_eq!(pod_name, "b");
                assert_eq!(session_id, s_b);
            }
            other => panic!("expected SessionConflict, got {other:?}"),
        }
    }
}
