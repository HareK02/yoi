//! Scope deny helpers for human-authored Workflow files.

use std::path::Path;

use manifest::{Permission, ScopeRule};
use memory::WorkspaceLayout;

/// Build deny rules that strip Write permission from
/// `<workspace>/.yoi/workflow/` for generic CRUD tools.
pub fn deny_write_rules(layout: &WorkspaceLayout) -> Vec<ScopeRule> {
    vec![deny_write(layout.workflow_dir().as_path())]
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
    fn deny_targets_workflow() {
        let layout = WorkspaceLayout::new(PathBuf::from("/ws"));
        let rules = deny_write_rules(&layout);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].target, PathBuf::from("/ws/.yoi/workflow"));
        assert_eq!(rules[0].permission, Permission::Write);
        assert!(rules[0].recursive);
    }
}
