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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;
    use crate::{ScopeLockError, delegate_scope, register_pod};
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
}
