//! Mutating operations over the allocation table. All of these expect
//! the caller to hold a [`LockFileGuard`] for the worker allocation's lock file.

use std::io;
use std::path::PathBuf;

use manifest::{DelegationScope, Permission, ScopeRule};
use session_store::SegmentId;

use super::conflict::{find_conflict_owner, find_conflict_owners};
use super::error::ScopeLockError;
use super::table::{Allocation, LockFileGuard};

/// Register a top-level Worker (started directly by a human, no
/// delegation parent). Reclaims stale entries before checking
/// conflicts so a crashed Worker's allocation doesn't block the new one.
///
/// Rejects when another live allocation is already writing to
/// `segment_id`, so two `restore_from_manifest` calls under different
/// `worker_name`s cannot both grab the same session log.
pub fn register_worker(
    guard: &mut LockFileGuard,
    worker_name: String,
    pid: u32,
    socket: PathBuf,
    scope_allow: Vec<ScopeRule>,
    segment_id: SegmentId,
) -> Result<(), ScopeLockError> {
    register_worker_with_deny(
        guard,
        worker_name,
        pid,
        socket,
        scope_allow,
        Vec::new(),
        segment_id,
    )
}

/// Register a top-level Worker with explicit deny rules that reduce the
/// claimed effective write scope.
///
/// Conflict semantics: if every Worker overlapping a requested allow rule
/// is fully covered by one of `scope_deny`, the conflict is suppressed
/// and the registration proceeds. The check is structural (deny ⊇
/// competitor.rule), not relational — it does not verify that the
/// competitor actually descends from this Worker's prior delegations.
/// In practice this is safe because the canonical restore caller derives
/// `scope_deny` from outstanding child worker metadata delegations, so any
/// covered competitor is expected to be a descendant of the original
/// allocation. Direct callers must uphold the same invariant.
pub fn register_worker_with_deny(
    guard: &mut LockFileGuard,
    worker_name: String,
    pid: u32,
    socket: PathBuf,
    scope_allow: Vec<ScopeRule>,
    scope_deny: Vec<ScopeRule>,
    segment_id: SegmentId,
) -> Result<(), ScopeLockError> {
    reclaim_stale(guard);
    if guard.data().find(&worker_name).is_some() {
        return Err(ScopeLockError::DuplicateWorkerName(worker_name));
    }
    if let Some(existing) = guard.data().find_by_segment(segment_id) {
        return Err(ScopeLockError::SegmentConflict {
            segment_id,
            worker_name: existing.worker_name.clone(),
            socket: existing.socket.clone(),
        });
    }
    for rule in scope_allow
        .iter()
        .filter(|r| r.permission == Permission::Write)
    {
        let conflicts = find_conflict_owners(guard.data(), rule, None);
        let all_denied = !conflicts.is_empty()
            && conflicts.iter().all(|owner| {
                scope_deny
                    .iter()
                    .filter(|r| r.permission == Permission::Write)
                    .any(|deny| super::conflict::covers_fully(deny, &owner.rule))
            });
        if all_denied {
            continue;
        }
        if let Some(competitor) = conflicts.into_iter().next() {
            return Err(ScopeLockError::WriteConflict {
                competitor: competitor.worker_name,
                rule: rule.clone(),
                competitor_rule: competitor.rule,
            });
        }
    }
    guard.data_mut().allocations.push(Allocation {
        worker_name,
        pid,
        socket,
        scope_allow,
        scope_deny,
        delegated_from: None,
        segment_id: Some(segment_id),
    });
    guard.save()?;
    Ok(())
}

/// Register a spawned Worker whose scope is delegated from `spawner`.
/// The requested scope must be within the spawner's delegation authority;
/// overlap with any Worker other than `spawner` is a conflict.
pub fn delegate_scope(
    guard: &mut LockFileGuard,
    spawner: &str,
    spawned: String,
    pid: u32,
    socket: PathBuf,
    scope_allow: Vec<ScopeRule>,
    delegation_scope: &DelegationScope,
) -> Result<(), ScopeLockError> {
    reclaim_stale(guard);
    if guard.data().find(&spawned).is_some() {
        return Err(ScopeLockError::DuplicateWorkerName(spawned));
    }
    if guard.data().find(spawner).is_none() {
        return Err(ScopeLockError::UnknownWorker(spawner.into()));
    }
    for rule in &scope_allow {
        let allowed = delegation_scope
            .allows_rule(rule)
            .map_err(|source| ScopeLockError::InvalidScope { source })?;
        if !allowed {
            return Err(ScopeLockError::NotSubset {
                spawner: spawner.into(),
                rule: rule.clone(),
            });
        }
        if rule.permission == Permission::Write {
            if let Some(competitor) = find_conflict_owner(guard.data(), rule, Some(spawner)) {
                return Err(ScopeLockError::WriteConflict {
                    competitor: competitor.worker_name,
                    rule: rule.clone(),
                    competitor_rule: competitor.rule,
                });
            }
        }
    }
    guard.data_mut().allocations.push(Allocation {
        worker_name: spawned,
        pid,
        socket,
        scope_allow,
        scope_deny: Vec::new(),
        delegated_from: Some(spawner.into()),
        // Pre-reservation. The child fills in its own segment_id when
        // it calls `adopt_allocation` after the worker is built.
        segment_id: None,
    });
    guard.save()?;
    Ok(())
}

/// Remove a Worker's allocation. Surviving children are reparented to
/// the removed Worker's own `delegated_from`, so the delegation tree
/// stays connected.
pub fn release_worker(guard: &mut LockFileGuard, worker_name: &str) -> Result<(), ScopeLockError> {
    let idx = guard
        .data()
        .allocations
        .iter()
        .position(|a| a.worker_name == worker_name);
    let Some(idx) = idx else {
        return Err(ScopeLockError::UnknownWorker(worker_name.into()));
    };
    let removed = guard.data().allocations[idx].clone();
    for alloc in guard.data_mut().allocations.iter_mut() {
        if alloc.delegated_from.as_deref() == Some(worker_name) {
            alloc.delegated_from.clone_from(&removed.delegated_from);
        }
    }
    guard.data_mut().allocations.remove(idx);
    guard.save()?;
    Ok(())
}

/// Reclaim a child delegation back into its parent allocation.
///
/// This is idempotent for missing deny entries. For each delegated Write rule,
/// at most one exact matching deny rule is removed from the parent's `scope_deny`
/// even when the child allocation is already absent; restore reconciliation uses
/// that case when durable Worker-state still records an outstanding delegation but
/// the live lock file no longer has a child allocation.
pub fn reclaim_delegated_scope(
    guard: &mut LockFileGuard,
    parent: &str,
    child: &str,
    delegated_scope: &[ScopeRule],
) -> Result<(), ScopeLockError> {
    let child_idx = guard
        .data()
        .allocations
        .iter()
        .position(|a| a.worker_name == child);
    let removed_child_parent = child_idx
        .map(|idx| guard.data().allocations[idx].delegated_from.clone())
        .unwrap_or(None);

    if let Some(parent_alloc) = guard.data_mut().find_mut(parent) {
        for rule in delegated_scope
            .iter()
            .filter(|rule| rule.permission == Permission::Write)
        {
            if let Some(idx) = parent_alloc.scope_deny.iter().position(|deny| deny == rule) {
                parent_alloc.scope_deny.remove(idx);
            }
        }
    }

    if let Some(idx) = child_idx {
        for alloc in guard.data_mut().allocations.iter_mut() {
            if alloc.delegated_from.as_deref() == Some(child) {
                alloc.delegated_from.clone_from(&removed_child_parent);
            }
        }
        guard.data_mut().allocations.remove(idx);
    }

    guard.save()?;
    Ok(())
}

/// Remove allocations whose PID is dead, reparenting children to the
/// dead Worker's `delegated_from`. Idempotent and best-effort — I/O
/// errors on save are swallowed so a crashed Worker's entry never blocks
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
        .map(|a| a.worker_name.clone())
        .collect();
    if dead.is_empty() {
        return;
    }
    for name in &dead {
        let Some(idx) = guard
            .data()
            .allocations
            .iter()
            .position(|a| a.worker_name == *name)
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
    use super::super::is_within_effective_write;
    use super::super::test_util::*;
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn register_detects_write_conflict() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        let err = register_worker(
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
    fn duplicate_worker_name_rejected() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        let err = register_worker(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a2"),
            vec![write_rule("/docs", true)],
            sid(),
        )
        .unwrap_err();
        assert!(matches!(err, ScopeLockError::DuplicateWorkerName(ref n) if n == "a"));
    }

    #[test]
    fn delegate_must_be_subset() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker(
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
            &delegation_scope(vec![write_rule("/src", true)]),
        )
        .unwrap_err();
        assert!(matches!(err, ScopeLockError::NotSubset { .. }));
    }

    #[test]
    fn delegate_uses_delegation_scope_not_direct_effective_write() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker(
            &mut g,
            "orchestrator".into(),
            std::process::id(),
            sock("orchestrator"),
            vec![read_rule("/workspace", true)],
            sid(),
        )
        .unwrap();

        delegate_scope(
            &mut g,
            "orchestrator",
            "coder".into(),
            std::process::id(),
            sock("coder"),
            vec![write_rule("/workspace/.worktree/task", true)],
            &delegation_scope(vec![write_rule("/workspace", true)]),
        )
        .unwrap();

        let coder = g.data().find("coder").expect("coder allocation");
        assert_eq!(coder.delegated_from.as_deref(), Some("orchestrator"));
    }

    #[test]
    fn delegate_succeeds_within_parent_scope() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker(
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
            &delegation_scope(vec![write_rule("/src", true)]),
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
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker(
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
            &delegation_scope(vec![write_rule("/src", true)]),
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
            &delegation_scope(vec![write_rule("/src", true)]),
        )
        .unwrap_err();
        match err {
            ScopeLockError::WriteConflict { competitor, .. } => assert_eq!(competitor, "b"),
            other => panic!("expected WriteConflict, got {other:?}"),
        }
    }

    #[test]
    fn release_reparents_children() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker(
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
            &delegation_scope(vec![write_rule("/src", true)]),
        )
        .unwrap();
        delegate_scope(
            &mut g,
            "b",
            "d".into(),
            std::process::id(),
            sock("d"),
            vec![write_rule("/src/core/x", true)],
            &delegation_scope(vec![write_rule("/src/core", true)]),
        )
        .unwrap();
        release_worker(&mut g, "b").unwrap();
        // D should now list A as its delegated_from.
        let d = g.data().find("d").unwrap();
        assert_eq!(d.delegated_from.as_deref(), Some("a"));
        assert!(g.data().find("b").is_none());
    }

    #[test]
    fn reclaim_delegated_scope_removes_child_and_one_parent_deny_layer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        let delegated_rule = write_rule("/src/core", true);
        register_worker_with_deny(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            vec![delegated_rule.clone(), delegated_rule.clone()],
            sid(),
        )
        .unwrap();
        register_worker(
            &mut g,
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![delegated_rule.clone()],
            sid(),
        )
        .unwrap();

        reclaim_delegated_scope(&mut g, "a", "b", std::slice::from_ref(&delegated_rule)).unwrap();
        let a = g.data().find("a").unwrap();
        assert_eq!(a.scope_deny, vec![delegated_rule.clone()]);
        assert!(g.data().find("b").is_none());

        reclaim_delegated_scope(&mut g, "a", "b", std::slice::from_ref(&delegated_rule)).unwrap();
        let a = g.data().find("a").unwrap();
        assert!(
            a.scope_deny.is_empty(),
            "a missing child allocation still reclaims one matching parent deny"
        );
    }

    #[test]
    fn reclaim_delegated_scope_removes_parent_deny_when_child_allocation_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        let delegated_rule = write_rule("/src/core", true);
        register_worker_with_deny(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            vec![delegated_rule.clone()],
            sid(),
        )
        .unwrap();

        reclaim_delegated_scope(
            &mut g,
            "a",
            "missing",
            std::slice::from_ref(&delegated_rule),
        )
        .unwrap();

        let a = g.data().find("a").unwrap();
        assert!(a.scope_deny.is_empty());
    }

    #[test]
    fn reclaim_stale_reparents_and_removes_dead_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker(
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
            &delegation_scope(vec![write_rule("/src", true)]),
        )
        .unwrap();
        delegate_scope(
            &mut g,
            "b",
            "d".into(),
            std::process::id(),
            sock("d"),
            vec![write_rule("/src/core/x", true)],
            &delegation_scope(vec![write_rule("/src/core", true)]),
        )
        .unwrap();
        // Simulate B crashing by rewriting its pid to one the probe
        // will treat as dead.
        let fake_dead_pid: u32 = 0xffff_fff0;
        for alloc in g.data_mut().allocations.iter_mut() {
            if alloc.worker_name == "b" {
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
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        // B only reads under the same tree — allowed.
        register_worker(
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
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker(
            &mut g,
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        release_worker(&mut g, "a").unwrap();
        register_worker(
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
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker(
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
            &delegation_scope(vec![write_rule("/src", true)]),
        )
        .unwrap();
        assert!(!is_within_effective_write(
            g.data(),
            "a",
            &write_rule("/src/core", true)
        ));
        release_worker(&mut g, "b").unwrap();
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
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        let shared_session = sid();
        register_worker(
            &mut g,
            "first".into(),
            std::process::id(),
            sock("first"),
            vec![write_rule("/work/a", true)],
            shared_session,
        )
        .unwrap();
        // Second registration tries to grab the same segment_id under
        // a different worker_name. Without the SegmentConflict check both
        // would succeed and race on the same jsonl.
        let err = register_worker(
            &mut g,
            "second".into(),
            std::process::id(),
            sock("second"),
            vec![write_rule("/work/b", true)],
            shared_session,
        )
        .unwrap_err();
        match err {
            ScopeLockError::SegmentConflict {
                segment_id,
                worker_name,
                ..
            } => {
                assert_eq!(segment_id, shared_session);
                assert_eq!(worker_name, "first");
            }
            other => panic!("expected SegmentConflict, got {other:?}"),
        }
    }
}
