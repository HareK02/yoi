//! Pure functions that decide whether scope rules collide.
//!
//! These helpers are read-only over [`LockFile`]; they never touch the
//! file or the lock itself. The mutating operations in [`crate::mutate`]
//! call them under the [`crate::LockFileGuard`].

use manifest::{Permission, ScopeRule};

use crate::table::{Allocation, LockFile};

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
pub(crate) fn covers_fully(cover: &ScopeRule, inner: &ScopeRule) -> bool {
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
/// scope: its allow set covers `rule`, no deny rule caps it, and no
/// child of `parent` has already taken a piece that would overlap
/// `rule`.
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
    let denied = alloc
        .scope_deny
        .iter()
        .filter(|r| r.permission == Permission::Write)
        .any(|r| rules_overlap(r, rule));
    if denied {
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

/// The Worker and rule that actually own a conflicting write scope.
#[derive(Debug, Clone)]
pub struct ConflictOwner {
    pub worker_name: String,
    pub rule: ScopeRule,
}

/// Find the Worker/rule that actually owns a write scope overlapping `rule`.
///
/// Walks the delegation tree: if an allocation overlaps `rule`, we
/// descend into its children and return the deepest overlapping node
/// as the true owner. `exempt` names a Worker whose ownership is
/// permitted (used during delegation: the spawner itself is allowed
/// to still own the rule's region because it is handing it down).
pub fn find_conflict_owner(
    lock: &LockFile,
    rule: &ScopeRule,
    exempt: Option<&str>,
) -> Option<ConflictOwner> {
    find_conflict_owners(lock, rule, exempt).into_iter().next()
}

/// Find every top-level delegation tree owner that conflicts with `rule`.
pub fn find_conflict_owners(
    lock: &LockFile,
    rule: &ScopeRule,
    exempt: Option<&str>,
) -> Vec<ConflictOwner> {
    if rule.permission != Permission::Write {
        return Vec::new();
    }
    lock.allocations
        .iter()
        .filter(|a| a.delegated_from.is_none())
        .filter_map(|alloc| find_conflict_in_subtree(lock, alloc, rule))
        .filter(|owner| Some(owner.worker_name.as_str()) != exempt)
        .collect()
}

fn find_conflict_in_subtree(
    lock: &LockFile,
    alloc: &Allocation,
    rule: &ScopeRule,
) -> Option<ConflictOwner> {
    let overlapping_rule = alloc
        .scope_allow
        .iter()
        .filter(|r| r.permission == Permission::Write)
        .find(|r| rules_overlap(r, rule))?;

    let fully_denied_here = alloc
        .scope_deny
        .iter()
        .filter(|r| r.permission == Permission::Write)
        .any(|r| covers_fully(r, rule));
    if fully_denied_here {
        return None;
    }

    for child in lock
        .allocations
        .iter()
        .filter(|a| a.delegated_from.as_deref() == Some(alloc.worker_name.as_str()))
    {
        if let Some(owner) = find_conflict_in_subtree(lock, child, rule) {
            return Some(owner);
        }
    }
    Some(ConflictOwner {
        worker_name: alloc.worker_name.clone(),
        rule: overlapping_rule.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;
    use crate::{ScopeLockError, delegate_scope, register_pod, register_worker_with_deny};
    use tempfile::TempDir;

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
    fn conflict_detection_descends_to_real_owner() {
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
        // A different top-level Worker trying to register /src/core/x
        // should be blamed on B (deepest owner), not A.
        let err = register_worker(
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
    fn denied_write_region_is_not_claimed_by_restored_parent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker_with_deny(
            &mut g,
            "parent".into(),
            std::process::id(),
            sock("parent"),
            vec![write_rule("/src", true)],
            vec![write_rule("/src/core", true)],
            sid(),
        )
        .unwrap();
        register_worker(
            &mut g,
            "child".into(),
            std::process::id(),
            sock("child"),
            vec![write_rule("/src/core", true)],
            sid(),
        )
        .unwrap();
    }

    #[test]
    fn partial_deny_does_not_hide_parent_conflict() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("workers.json");
        let mut g = open_empty(&path);
        register_worker_with_deny(
            &mut g,
            "parent".into(),
            std::process::id(),
            sock("parent"),
            vec![write_rule("/src", true)],
            vec![write_rule("/src/core", true)],
            sid(),
        )
        .unwrap();

        let err = register_worker(
            &mut g,
            "other".into(),
            std::process::id(),
            sock("other"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap_err();

        match err {
            ScopeLockError::WriteConflict {
                competitor,
                competitor_rule,
                ..
            } => {
                assert_eq!(competitor, "parent");
                assert_eq!(competitor_rule.target, std::path::PathBuf::from("/src"));
            }
            other => panic!("expected WriteConflict, got {other:?}"),
        }
    }
}
