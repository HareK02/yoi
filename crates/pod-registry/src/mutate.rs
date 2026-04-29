//! Mutating operations over the allocation table. All of these expect
//! the caller to hold a [`LockFileGuard`] for the registry's lock file.

use std::io;
use std::path::PathBuf;

use manifest::{Permission, ScopeRule};
use session_store::SessionId;

use crate::conflict::{find_conflict_owner, is_within_effective_write};
use crate::error::ScopeLockError;
use crate::table::{Allocation, LockFileGuard};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::is_within_effective_write;
    use crate::test_util::*;
    use tempfile::TempDir;

    #[test]
    fn register_detects_write_conflict() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pods.json");
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
        let path = dir.path().join("pods.json");
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
        let path = dir.path().join("pods.json");
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
        let path = dir.path().join("pods.json");
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
        let path = dir.path().join("pods.json");
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
        let path = dir.path().join("pods.json");
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
        let path = dir.path().join("pods.json");
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

    #[test]
    fn read_rules_do_not_conflict_with_write() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pods.json");
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
        let path = dir.path().join("pods.json");
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
        let path = dir.path().join("pods.json");
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
    fn register_pod_rejects_session_id_collision() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pods.json");
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
}
