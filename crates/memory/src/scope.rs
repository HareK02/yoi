//! Helpers for constructing `ScopeRule` entries that exclude the
//! memory tree from the generic CRUD tools' write surface.
//!
//! Pod is expected to call [`deny_write_rules`] when memory is enabled
//! and append the result to the manifest's `scope.deny` list before
//! constructing the [`Scope`] passed to `tools::ScopedFs`. The memory
//! tools themselves bypass `ScopedFs` and write directly under the
//! workspace root, so this deny does not affect their operation.

use std::path::Path;

use manifest::{Permission, ScopeRule};

use crate::workspace::WorkspaceLayout;

/// Build deny rules that strip Write permission from `<workspace>/memory/`
/// and `<workspace>/knowledge/`. Recursive — every descendant is capped at
/// Read for the generic tools.
pub fn deny_write_rules(layout: &WorkspaceLayout) -> Vec<ScopeRule> {
    vec![
        deny_write(layout.memory_dir().as_path()),
        deny_write(layout.knowledge_dir().as_path()),
    ]
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
    fn deny_targets_memory_and_knowledge() {
        let layout = WorkspaceLayout::new(PathBuf::from("/ws"));
        let rules = deny_write_rules(&layout);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].target, PathBuf::from("/ws/.yoi/memory"));
        assert_eq!(rules[0].permission, Permission::Write);
        assert!(rules[0].recursive);
        assert_eq!(rules[1].target, PathBuf::from("/ws/.yoi/knowledge"));
    }
}
