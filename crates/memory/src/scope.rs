//! Helpers for constructing `ScopeRule` entries that exclude the
//! memory tree from the generic CRUD tools' write surface.
//!
//! Worker is expected to call [`deny_write_rules`] when memory is enabled
//! and append the result to the manifest's `scope.deny` list before
//! constructing the [`Scope`] passed to the local Workdir provider. The
//! memory tools themselves bypass generic Workdir filesystem operations and
//! write directly under the workspace root, so this deny does not affect them.

use std::path::Path;

use manifest::{Permission, ScopeRule};

use crate::workspace::WorkspaceLayout;

/// Build a deny rule that strips Write permission from `<workspace>/.yoi/memory/`.
pub fn deny_write_rules(layout: &WorkspaceLayout) -> Vec<ScopeRule> {
    vec![deny_write(layout.memory_dir().as_path())]
}

fn deny_write(target: &Path) -> ScopeRule {
    ScopeRule {
        target: target.to_path_buf(),
        permission: Permission::Write,
        recursive: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn deny_targets_memory() {
        let layout = WorkspaceLayout::new(PathBuf::from("/ws"));
        let rules = deny_write_rules(&layout);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].target, PathBuf::from("/ws/.yoi/memory"));
        assert_eq!(rules[0].permission, Permission::Write);
        assert!(rules[0].recursive);
    }
}
