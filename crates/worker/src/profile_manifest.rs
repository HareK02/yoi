//! Runtime manifest preparation from Backend-resolved profiles.
//!
//! This module has no process entrypoint or local profile discovery authority.

use std::path::{Path, PathBuf};

use crate::PromptCatalogSource;
use manifest::{Permission, ScopeConfig, ScopeRule, WorkerManifest};

pub fn resolve_runtime_profile_manifest_from_manifest(
    mut manifest: WorkerManifest,
    workspace_root: &Path,
    worker_name: &str,
) -> Result<(WorkerManifest, PromptCatalogSource), String> {
    if manifest.worker.name.is_empty() {
        manifest.worker.name = worker_name.to_string();
    }
    apply_profile_launch_policy(&mut manifest, workspace_root);
    // Do not run plugin discovery here: runtime-created Workers receive their
    // resolved manifest/profile archive from Backend authority, not by scanning
    // materialized workdir-local plugin stores.
    Ok((manifest, PromptCatalogSource::builtins_only()))
}

pub fn resolve_runtime_profile_manifest_from_manifest_without_filesystem(
    mut manifest: WorkerManifest,
    _workspace_root: &Path,
    worker_name: &str,
) -> Result<(WorkerManifest, PromptCatalogSource), String> {
    if manifest.worker.name.is_empty() {
        manifest.worker.name = worker_name.to_string();
    }
    manifest.scope = ScopeConfig::default();
    manifest.delegation_scope = ScopeConfig::default();
    // Same as the filesystem-capable runtime path: no local discovery.
    Ok((manifest, PromptCatalogSource::builtins_only()))
}

fn read_rule(target: PathBuf) -> ScopeRule {
    ScopeRule {
        target,
        permission: Permission::Read,
        recursive: true,
        symlink_policy: Default::default(),
    }
}

fn write_rule(target: PathBuf) -> ScopeRule {
    ScopeRule {
        target,
        permission: Permission::Write,
        recursive: true,
        symlink_policy: Default::default(),
    }
}

fn workspace_scope(
    workspace_root: &Path,
    permission: Permission,
    deny_write: &[PathBuf],
) -> ScopeConfig {
    let allow_rule = ScopeRule {
        target: workspace_root.to_path_buf(),
        permission,
        recursive: true,
        symlink_policy: Default::default(),
    };
    let deny = deny_write
        .iter()
        .cloned()
        .map(write_rule)
        .collect::<Vec<_>>();
    ScopeConfig {
        allow: vec![allow_rule],
        deny,
    }
}

fn workspace_worktree_delegation(workspace_root: &Path) -> ScopeConfig {
    ScopeConfig {
        allow: vec![
            read_rule(workspace_root.to_path_buf()),
            write_rule(workspace_root.join(".worktree")),
        ],
        deny: Vec::new(),
    }
}

fn append_missing_rules(target: &mut Vec<ScopeRule>, defaults: Vec<ScopeRule>) {
    for rule in defaults {
        if !target.contains(&rule) {
            target.push(rule);
        }
    }
}

fn apply_scope_launch_defaults(scope: &mut ScopeConfig, defaults: ScopeConfig) {
    // Profile resolution has already applied explicit profile/workspace override scope rules.
    // Launch policy contributes runtime defaults on top rather than replacing those grants.
    append_missing_rules(&mut scope.allow, defaults.allow);
    append_missing_rules(&mut scope.deny, defaults.deny);
}

fn apply_profile_launch_policy(manifest: &mut WorkerManifest, workspace_root: &Path) {
    let worktree_root = workspace_root.join(".worktree");
    let default_scope = workspace_scope(
        workspace_root,
        Permission::Write,
        std::slice::from_ref(&worktree_root),
    );
    apply_scope_launch_defaults(&mut manifest.scope, default_scope);
    manifest.delegation_scope = workspace_worktree_delegation(workspace_root);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> WorkerManifest {
        let mut config = manifest::WorkerManifestConfig::resolution_defaults();
        config.worker.name = Some("resolved".into());
        WorkerManifest::try_from(config).unwrap()
    }

    #[test]
    fn runtime_launch_preserves_explicit_scope_without_repository_discovery() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".yoi")).unwrap();
        std::fs::write(root.path().join(".yoi/manifest.toml"), "invalid TOML").unwrap();
        let mut original = manifest();
        let explicit = read_rule(root.path().join("reference"));
        original.scope.allow.push(explicit.clone());
        let (resolved, _) =
            resolve_runtime_profile_manifest_from_manifest(original, root.path(), "fallback")
                .unwrap();
        assert_eq!(resolved.worker.name, "resolved");
        assert!(resolved.scope.allow.contains(&explicit));
        assert!(
            resolved
                .scope
                .allow
                .contains(&write_rule(root.path().to_path_buf()))
        );
        assert!(
            resolved
                .scope
                .deny
                .contains(&write_rule(root.path().join(".worktree")))
        );
        assert_eq!(
            resolved.delegation_scope.allow,
            vec![
                read_rule(root.path().to_path_buf()),
                write_rule(root.path().join(".worktree")),
            ]
        );
    }

    #[test]
    fn runtime_without_filesystem_clears_scope_and_delegation_grants() {
        let mut original = manifest();
        original.worker.name.clear();
        original
            .scope
            .allow
            .push(write_rule(PathBuf::from("/work")));
        original
            .delegation_scope
            .allow
            .push(write_rule(PathBuf::from("/work")));
        let (resolved, _) = resolve_runtime_profile_manifest_from_manifest_without_filesystem(
            original,
            Path::new("/unused"),
            "runtime-worker",
        )
        .unwrap();
        assert_eq!(resolved.worker.name, "runtime-worker");
        assert!(resolved.scope.allow.is_empty());
        assert!(resolved.scope.deny.is_empty());
        assert!(resolved.delegation_scope.allow.is_empty());
        assert!(resolved.delegation_scope.deny.is_empty());
    }
}
