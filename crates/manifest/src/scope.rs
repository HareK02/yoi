//! Runtime representation of a Pod's access scope.
//!
//! Built from [`crate::ScopeConfig`] via [`Scope::from_config`]. Every
//! rule `target` must already be an absolute path — per-layer path
//! resolution runs earlier, inside [`crate::PodManifestConfig::resolve_paths`].
//! All rule `target` paths inside the [`Scope`] are canonicalised (where
//! possible) so access checks are pure path comparisons.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arc_swap::{ArcSwap, Guard};

use crate::{Permission, ScopeConfig, ScopeRule};

/// Parsed, pwd-resolved set of allow/deny rules for a Pod.
///
/// Read/write access decisions are pure functions of the path being
/// queried and these rules — see [`Scope::permission_at`].
#[derive(Debug, Clone)]
pub struct Scope {
    allow: Vec<ResolvedRule>,
    deny: Vec<ResolvedRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedRule {
    /// Absolute, canonicalized-or-normalized target directory/file.
    target: PathBuf,
    permission: Permission,
    recursive: bool,
}

/// Errors raised when constructing a [`Scope`] from a [`ScopeConfig`].
#[derive(Debug, thiserror::Error)]
pub enum ScopeError {
    #[error("scope must declare at least one [[scope.allow]] rule")]
    EmptyAllow,
    #[error("scope target must be absolute: {}", .0.display())]
    RelativeTarget(PathBuf),
    #[error("failed to resolve scope target {}: {source}", .path.display())]
    ResolveTarget {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Scope {
    /// Build a [`Scope`] from a declarative [`ScopeConfig`].
    ///
    /// Every `target` in `config` must already be absolute — per-layer
    /// resolution happens upstream in
    /// [`crate::PodManifestConfig::resolve_paths`] so that cascade merge
    /// operates on fully-qualified paths. A lingering relative target
    /// here signals an upstream bug and is rejected.
    pub fn from_config(config: &ScopeConfig) -> Result<Self, ScopeError> {
        if config.allow.is_empty() {
            return Err(ScopeError::EmptyAllow);
        }
        let allow = config
            .allow
            .iter()
            .map(resolve_rule)
            .collect::<Result<Vec<_>, _>>()?;
        let deny = config
            .deny
            .iter()
            .map(resolve_rule)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { allow, deny })
    }

    /// Convenience constructor for tests and simple setups: a single
    /// recursive `allow(Write)` rule rooted at `root`.
    pub fn writable(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = root.as_ref().canonicalize()?;
        Ok(Self {
            allow: vec![ResolvedRule {
                target: root,
                permission: Permission::Write,
                recursive: true,
            }],
            deny: Vec::new(),
        })
    }

    /// Effective permission for `path`.
    ///
    /// Returns `None` when `path` is outside every allow rule, or when
    /// deny rules have knocked it below `Read`.
    pub fn permission_at(&self, path: &Path) -> Option<Permission> {
        let resolved = resolve_path(path)?;
        let mut effective: Option<Permission> = None;
        for rule in &self.allow {
            if rule.matches(&resolved) {
                effective = match effective {
                    None => Some(rule.permission),
                    Some(cur) => Some(cur.max(rule.permission)),
                };
            }
        }
        let mut effective = effective?;

        // Deny: min(min_deny) dictates the cap. Effective level is capped
        // strictly below that value, so deny(read) wipes access entirely.
        let mut min_deny: Option<Permission> = None;
        for rule in &self.deny {
            if rule.matches(&resolved) {
                min_deny = match min_deny {
                    None => Some(rule.permission),
                    Some(cur) => Some(cur.min(rule.permission)),
                };
            }
        }
        if let Some(cap) = min_deny {
            match cap {
                Permission::Read => return None,
                Permission::Write => effective = effective.min(Permission::Read),
            }
        }
        Some(effective)
    }

    /// Shorthand: `permission_at(path) >= Some(Read)`.
    pub fn is_readable(&self, path: &Path) -> bool {
        matches!(
            self.permission_at(path),
            Some(Permission::Read | Permission::Write)
        )
    }

    /// Shorthand: `permission_at(path) == Some(Write)`.
    pub fn is_writable(&self, path: &Path) -> bool {
        matches!(self.permission_at(path), Some(Permission::Write))
    }

    /// Iterate over absolute paths granted at least `Read` by an allow
    /// rule, preserving declaration order. Does not account for deny
    /// rules, which only cap effective permission at query time.
    pub fn readable_paths(&self) -> impl Iterator<Item = &Path> {
        self.allow.iter().map(|r| r.target.as_path())
    }

    /// Allow rules with their targets resolved to absolute paths.
    ///
    /// Used by the pod-registry, where every Pod's allocation
    /// must be expressed in absolute terms so prefix comparisons are
    /// meaningful across processes.
    pub fn allow_rules(&self) -> Vec<ScopeRule> {
        self.allow
            .iter()
            .map(|r| ScopeRule {
                target: r.target.clone(),
                permission: r.permission,
                recursive: r.recursive,
            })
            .collect()
    }

    /// Deny rules with their targets resolved to absolute paths.
    ///
    /// Counterpart to [`allow_rules`](Self::allow_rules); together they
    /// round-trip through [`ScopeConfig`] for callers that need to
    /// rebuild a scope after layering extra rules on top of an
    /// already-constructed [`Scope`].
    pub fn deny_rules(&self) -> Vec<ScopeRule> {
        self.deny
            .iter()
            .map(|r| ScopeRule {
                target: r.target.clone(),
                permission: r.permission,
                recursive: r.recursive,
            })
            .collect()
    }

    /// Iterate over absolute paths granted `Write` by an allow rule.
    /// Subset of [`readable_paths`](Self::readable_paths).
    pub fn writable_paths(&self) -> impl Iterator<Item = &Path> {
        self.allow
            .iter()
            .filter(|r| r.permission == Permission::Write)
            .map(|r| r.target.as_path())
    }

    /// Build a new [`Scope`] equal to `self` with `extra_allow` appended
    /// to the allow set. Used by dynamic-scope grow paths
    /// (e.g. controller adding the bash-output Read rule, future
    /// external `GrantScope`).
    pub fn with_added_allow_rules(
        &self,
        extra_allow: impl IntoIterator<Item = ScopeRule>,
    ) -> Result<Self, ScopeError> {
        let mut config = ScopeConfig {
            allow: self.allow_rules(),
            deny: self.deny_rules(),
        };
        config.allow.extend(extra_allow);
        Self::from_config(&config)
    }

    /// Build a new [`Scope`] equal to `self` with `extra_deny` appended
    /// to the deny set. Used by dynamic-scope shrink paths
    /// (e.g. SpawnPod-style delegation that strips Write from the
    /// spawner without touching its allow rules).
    pub fn with_added_deny_rules(
        &self,
        extra_deny: impl IntoIterator<Item = ScopeRule>,
    ) -> Result<Self, ScopeError> {
        let mut config = ScopeConfig {
            allow: self.allow_rules(),
            deny: self.deny_rules(),
        };
        config.deny.extend(extra_deny);
        Self::from_config(&config)
    }

    /// Build a new [`Scope`] with one matching deny rule removed for each
    /// rule in `remove_deny`.
    ///
    /// This is intentionally exact (after the same target resolution used
    /// by [`Scope::from_config`]) rather than geometric: reclaiming a
    /// delegated child must remove the deny layer that was added for that
    /// child without broadening any explicit base deny that merely overlaps
    /// the delegated path. Missing rules are ignored, making repeated
    /// reclaim calls harmless.
    pub fn with_removed_deny_rules(
        &self,
        remove_deny: impl IntoIterator<Item = ScopeRule>,
    ) -> Result<Self, ScopeError> {
        let mut deny = self.deny.clone();
        for rule in remove_deny {
            let resolved = resolve_rule(&rule)?;
            if let Some(idx) = deny.iter().position(|existing| existing == &resolved) {
                deny.remove(idx);
            }
        }
        Ok(Self {
            allow: self.allow.clone(),
            deny,
        })
    }

    /// Human-readable grouping of allow rules, suitable for embedding in
    /// LLM system prompts. Deny rules are intentionally omitted — they
    /// only cap effective permission and surface them would mislead the
    /// reader about what paths are accessible. Rules with
    /// `recursive = false` are tagged with a trailing `[non-recursive]`
    /// marker so the model does not assume child paths are included.
    ///
    /// ```text
    /// Readable:
    ///   - /abs/path1 [non-recursive]
    /// Writable:
    ///   - /abs/path2
    /// ```
    pub fn summary(&self) -> String {
        fn push_rule(out: &mut String, rule: &ResolvedRule) {
            out.push_str("  - ");
            out.push_str(&rule.target.display().to_string());
            if !rule.recursive {
                out.push_str(" [non-recursive]");
            }
            out.push('\n');
        }

        let mut out = String::new();
        if !self.allow.is_empty() {
            out.push_str("Readable:\n");
            for rule in &self.allow {
                push_rule(&mut out, rule);
            }
        }
        let writable: Vec<&ResolvedRule> = self
            .allow
            .iter()
            .filter(|r| r.permission == Permission::Write)
            .collect();
        if !writable.is_empty() {
            out.push_str("Writable:\n");
            for rule in &writable {
                push_rule(&mut out, rule);
            }
        }
        if out.ends_with('\n') {
            out.pop();
        }
        out
    }
}

/// Shared, atomically-swappable view of a [`Scope`].
///
/// Built around [`ArcSwap`] so the hot path (permission checks inside
/// `ScopedFs`) reads the current scope lock-free. Mutators are
/// serialised by an internal `Mutex` so concurrent `update` calls do
/// not lose each other's contributions.
///
/// All clones share the same underlying state — a `SharedScope` cloned
/// out to multiple consumers (Pod, ScopedFs, future grant/revoke
/// callers) sees every update.
#[derive(Debug, Clone)]
pub struct SharedScope {
    inner: Arc<SharedScopeInner>,
}

#[derive(Debug)]
struct SharedScopeInner {
    scope: ArcSwap<Scope>,
    /// Serialises read-modify-write update transactions so a producer
    /// can read the current scope, build a derived one, and store it
    /// without losing concurrent updates.
    write_lock: Mutex<()>,
}

impl SharedScope {
    /// Wrap an owned [`Scope`] in a shared, atomically-swappable handle.
    pub fn new(scope: Scope) -> Self {
        Self {
            inner: Arc::new(SharedScopeInner {
                scope: ArcSwap::from_pointee(scope),
                write_lock: Mutex::new(()),
            }),
        }
    }

    /// Snapshot the current scope. Cheap and lock-free; the returned
    /// guard borrows the live scope for as long as it is held.
    pub fn load(&self) -> Guard<Arc<Scope>> {
        self.inner.scope.load()
    }

    /// Snapshot the current scope into an owned `Arc<Scope>`. Useful
    /// when the caller needs a value that outlives the load guard
    /// (e.g. cloning into another struct).
    pub fn snapshot(&self) -> Arc<Scope> {
        self.inner.scope.load_full()
    }

    /// Read-modify-write transaction. `f` is called with the current
    /// scope and returns a derived one (or an error). The internal
    /// write lock ensures that two concurrent `update` calls see each
    /// other's results — the second observes the first's output as its
    /// input.
    pub fn update<F>(&self, f: F) -> Result<(), ScopeError>
    where
        F: FnOnce(&Scope) -> Result<Scope, ScopeError>,
    {
        let _guard = self.inner.write_lock.lock().expect("scope mutex poisoned");
        let current = self.inner.scope.load();
        let new = f(&current)?;
        self.inner.scope.store(Arc::new(new));
        Ok(())
    }
}

impl ResolvedRule {
    fn matches(&self, path: &Path) -> bool {
        if self.recursive {
            path.starts_with(&self.target)
        } else {
            path == self.target || path.parent() == Some(self.target.as_path())
        }
    }
}

fn resolve_rule(rule: &ScopeRule) -> Result<ResolvedRule, ScopeError> {
    if !rule.target.is_absolute() {
        return Err(ScopeError::RelativeTarget(rule.target.clone()));
    }
    let target = resolve_path(&rule.target).ok_or_else(|| ScopeError::ResolveTarget {
        path: rule.target.clone(),
        source: std::io::Error::new(std::io::ErrorKind::Other, "could not absolutize target"),
    })?;
    Ok(ResolvedRule {
        target,
        permission: rule.permission,
        recursive: rule.recursive,
    })
}

/// Convert `path` to an absolute form suitable for prefix comparison.
///
/// Tries `canonicalize` on the full path first (resolves symlinks). If
/// the path doesn't exist yet, climbs to the closest existing ancestor,
/// canonicalizes it, then rejoins the missing tail. Returns `None` for
/// relative inputs that have no existing ancestor to anchor against.
fn resolve_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    if let Ok(canonical) = path.canonicalize() {
        return Some(canonical);
    }
    let mut tail: Vec<OsString> = Vec::new();
    let mut cur = path.to_path_buf();
    loop {
        if let Ok(canonical) = cur.canonicalize() {
            let mut out = canonical;
            for segment in tail.iter().rev() {
                out.push(segment);
            }
            return Some(out);
        }
        let name = cur.file_name()?.to_os_string();
        tail.push(name);
        let parent = cur.parent()?.to_path_buf();
        if parent == cur {
            return None;
        }
        cur = parent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn allow_rule(target: &Path, permission: Permission) -> ScopeRule {
        ScopeRule {
            target: target.to_path_buf(),
            permission,
            recursive: true,
        }
    }

    #[test]
    fn writable_shortcut_permits_root() {
        let dir = TempDir::new().unwrap();
        let scope = Scope::writable(dir.path()).unwrap();
        assert!(scope.is_writable(&dir.path().join("a.txt")));
        assert!(scope.is_readable(&dir.path().join("a.txt")));
    }

    #[test]
    fn writable_shortcut_rejects_outside() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let scope = Scope::writable(dir.path()).unwrap();
        assert!(!scope.is_readable(&outside.path().join("x")));
    }

    #[test]
    fn allow_write_grants_read_and_write() {
        let dir = TempDir::new().unwrap();
        let cfg = ScopeConfig {
            allow: vec![allow_rule(dir.path(), Permission::Write)],
            deny: Vec::new(),
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let f = dir.path().join("a.txt");
        assert_eq!(scope.permission_at(&f), Some(Permission::Write));
    }

    #[test]
    fn allow_read_only() {
        let dir = TempDir::new().unwrap();
        let cfg = ScopeConfig {
            allow: vec![allow_rule(dir.path(), Permission::Read)],
            deny: Vec::new(),
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let f = dir.path().join("a.txt");
        assert_eq!(scope.permission_at(&f), Some(Permission::Read));
        assert!(scope.is_readable(&f));
        assert!(!scope.is_writable(&f));
    }

    #[test]
    fn deny_write_downgrades_to_read() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let cfg = ScopeConfig {
            allow: vec![allow_rule(dir.path(), Permission::Write)],
            deny: vec![allow_rule(&sub, Permission::Write)],
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let f = sub.join("a.txt");
        assert_eq!(scope.permission_at(&f), Some(Permission::Read));
        // outside the deny, still writable.
        assert_eq!(
            scope.permission_at(&dir.path().join("top.txt")),
            Some(Permission::Write)
        );
    }

    #[test]
    fn deny_read_removes_access_entirely() {
        let dir = TempDir::new().unwrap();
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, b"").unwrap();
        let cfg = ScopeConfig {
            allow: vec![allow_rule(dir.path(), Permission::Write)],
            deny: vec![allow_rule(&secret, Permission::Read)],
        };
        let scope = Scope::from_config(&cfg).unwrap();
        assert_eq!(scope.permission_at(&secret), None);
    }

    #[test]
    fn multiple_allow_rules_take_max() {
        let dir = TempDir::new().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir(&docs).unwrap();
        let cfg = ScopeConfig {
            allow: vec![
                allow_rule(dir.path(), Permission::Read),
                allow_rule(&docs, Permission::Write),
            ],
            deny: Vec::new(),
        };
        let scope = Scope::from_config(&cfg).unwrap();
        assert_eq!(
            scope.permission_at(&dir.path().join("a.txt")),
            Some(Permission::Read)
        );
        assert_eq!(
            scope.permission_at(&docs.join("a.txt")),
            Some(Permission::Write)
        );
    }

    #[test]
    fn non_recursive_rule_matches_direct_children_only() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        let cfg = ScopeConfig {
            allow: vec![ScopeRule {
                target: dir.path().to_path_buf(),
                permission: Permission::Write,
                recursive: false,
            }],
            deny: Vec::new(),
        };
        let scope = Scope::from_config(&cfg).unwrap();
        assert!(scope.is_writable(&dir.path().join("top.txt")));
        assert!(!scope.is_writable(&nested.join("deep.txt")));
    }

    #[test]
    fn empty_allow_rejected() {
        let cfg = ScopeConfig {
            allow: Vec::new(),
            deny: Vec::new(),
        };
        let err = Scope::from_config(&cfg).unwrap_err();
        assert!(matches!(err, ScopeError::EmptyAllow));
    }

    #[test]
    fn relative_target_rejected_as_invariant_violation() {
        let cfg = ScopeConfig {
            allow: vec![ScopeRule {
                target: PathBuf::from("relative/path"),
                permission: Permission::Read,
                recursive: true,
            }],
            deny: Vec::new(),
        };
        let err = Scope::from_config(&cfg).unwrap_err();
        assert!(matches!(err, ScopeError::RelativeTarget(_)));
    }

    #[test]
    fn rejects_traversal_attack() {
        let dir = TempDir::new().unwrap();
        let scope = Scope::writable(dir.path()).unwrap();
        let traversal = dir.path().join("../../../etc/passwd");
        assert!(!scope.is_readable(&traversal));
    }

    #[test]
    fn summary_lists_readable_and_writable() {
        let dir = TempDir::new().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir(&docs).unwrap();
        let cfg = ScopeConfig {
            allow: vec![
                allow_rule(dir.path(), Permission::Read),
                allow_rule(&docs, Permission::Write),
            ],
            deny: Vec::new(),
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let summary = scope.summary();
        assert!(summary.contains("Readable:"));
        assert!(summary.contains("Writable:"));
        assert!(summary.contains(&dir.path().canonicalize().unwrap().display().to_string()));
        assert!(summary.contains(&docs.canonicalize().unwrap().display().to_string()));
        assert!(!summary.ends_with('\n'));
    }

    #[test]
    fn summary_excludes_deny_rules() {
        let dir = TempDir::new().unwrap();
        let secret = dir.path().join("secret");
        std::fs::create_dir(&secret).unwrap();
        let cfg = ScopeConfig {
            allow: vec![allow_rule(dir.path(), Permission::Write)],
            deny: vec![allow_rule(&secret, Permission::Read)],
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let summary = scope.summary();
        assert!(!summary.contains("secret"));
    }

    #[test]
    fn summary_marks_non_recursive_rules() {
        let dir = TempDir::new().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir(&docs).unwrap();
        let cfg = ScopeConfig {
            allow: vec![
                ScopeRule {
                    target: docs.clone(),
                    permission: Permission::Read,
                    recursive: false,
                },
                ScopeRule {
                    target: dir.path().to_path_buf(),
                    permission: Permission::Write,
                    recursive: true,
                },
            ],
            deny: Vec::new(),
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let summary = scope.summary();
        let docs_canon = docs.canonicalize().unwrap().display().to_string();
        let dir_canon = dir.path().canonicalize().unwrap().display().to_string();
        assert!(
            summary.contains(&format!("{docs_canon} [non-recursive]")),
            "expected non-recursive marker in: {summary}"
        );
        // Recursive rule must NOT carry the marker.
        assert!(
            !summary.contains(&format!("{dir_canon} [non-recursive]")),
            "recursive rule incorrectly marked: {summary}"
        );
    }

    #[test]
    fn readable_paths_includes_writable() {
        let dir = TempDir::new().unwrap();
        let docs = dir.path().join("docs");
        std::fs::create_dir(&docs).unwrap();
        let cfg = ScopeConfig {
            allow: vec![
                allow_rule(dir.path(), Permission::Read),
                allow_rule(&docs, Permission::Write),
            ],
            deny: Vec::new(),
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let readable: Vec<_> = scope.readable_paths().collect();
        let writable: Vec<_> = scope.writable_paths().collect();
        assert_eq!(readable.len(), 2);
        assert_eq!(writable.len(), 1);
        assert!(writable.iter().all(|w| readable.contains(w)));
    }

    #[test]
    fn resolves_new_nested_file_inside_scope() {
        let dir = TempDir::new().unwrap();
        let scope = Scope::writable(dir.path()).unwrap();
        let deep = dir.path().join("a/b/c/new.txt");
        assert!(scope.is_writable(&deep));
    }

    #[test]
    fn with_added_allow_rules_grows_readable_set() {
        let dir = TempDir::new().unwrap();
        let extra = TempDir::new().unwrap();
        let base = Scope::writable(dir.path()).unwrap();
        assert!(!base.is_readable(&extra.path().join("x")));
        let extended = base
            .with_added_allow_rules([ScopeRule {
                target: extra.path().to_path_buf(),
                permission: Permission::Read,
                recursive: true,
            }])
            .unwrap();
        assert!(extended.is_readable(&extra.path().join("x")));
        assert!(extended.is_writable(&dir.path().join("y")));
    }

    #[test]
    fn with_added_deny_rules_demotes_write_to_read() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let base = Scope::writable(dir.path()).unwrap();
        let demoted = base
            .with_added_deny_rules([ScopeRule {
                target: sub.clone(),
                permission: Permission::Write,
                recursive: true,
            }])
            .unwrap();
        let f = sub.join("a.txt");
        assert_eq!(demoted.permission_at(&f), Some(Permission::Read));
        assert_eq!(
            demoted.permission_at(&dir.path().join("top.txt")),
            Some(Permission::Write)
        );
    }

    #[test]
    fn with_removed_deny_rules_reclaims_one_matching_layer() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let rule = ScopeRule {
            target: sub.clone(),
            permission: Permission::Write,
            recursive: true,
        };
        let base = Scope::writable(dir.path())
            .unwrap()
            .with_added_deny_rules([rule.clone(), rule.clone()])
            .unwrap();

        let reclaimed_once = base.with_removed_deny_rules([rule.clone()]).unwrap();
        assert_eq!(
            reclaimed_once.permission_at(&sub.join("a.txt")),
            Some(Permission::Read),
            "one duplicate deny layer must remain"
        );

        let reclaimed_twice = reclaimed_once
            .with_removed_deny_rules([rule.clone()])
            .unwrap();
        assert_eq!(
            reclaimed_twice.permission_at(&sub.join("a.txt")),
            Some(Permission::Write)
        );

        let reclaimed_again = reclaimed_twice.with_removed_deny_rules([rule]).unwrap();
        assert_eq!(
            reclaimed_again.permission_at(&sub.join("a.txt")),
            Some(Permission::Write),
            "missing rules are ignored for idempotent reclaim"
        );
    }

    #[test]
    fn shared_scope_load_returns_current_value() {
        let dir = TempDir::new().unwrap();
        let shared = SharedScope::new(Scope::writable(dir.path()).unwrap());
        assert!(shared.load().is_writable(&dir.path().join("a.txt")));
    }

    #[test]
    fn shared_scope_update_replaces_view_atomically() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let shared = SharedScope::new(Scope::writable(dir.path()).unwrap());
        let target = sub.join("a.txt");
        assert_eq!(
            shared.load().permission_at(&target),
            Some(Permission::Write)
        );
        shared
            .update(|cur| {
                cur.with_added_deny_rules([ScopeRule {
                    target: sub.clone(),
                    permission: Permission::Write,
                    recursive: true,
                }])
            })
            .unwrap();
        assert_eq!(shared.load().permission_at(&target), Some(Permission::Read));
    }

    #[test]
    fn shared_scope_clones_share_state() {
        let dir = TempDir::new().unwrap();
        let extra = TempDir::new().unwrap();
        let a = SharedScope::new(Scope::writable(dir.path()).unwrap());
        let b = a.clone();
        assert!(!b.load().is_readable(&extra.path().join("x")));
        a.update(|cur| {
            cur.with_added_allow_rules([ScopeRule {
                target: extra.path().to_path_buf(),
                permission: Permission::Read,
                recursive: true,
            }])
        })
        .unwrap();
        assert!(b.load().is_readable(&extra.path().join("x")));
    }
}
