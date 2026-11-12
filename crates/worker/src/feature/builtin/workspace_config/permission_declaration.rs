//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "workspace-config",
    namespace: "workspace_config",
    operations: &[
        BuiltinPermissionOperation {
            key: "workspace_config.attach",
            description: "Attach the selected configuration path and recursive scope.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workspace_config.observe",
            description: "Observe selected configuration metadata within path scope.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workspace_config.read",
            description: "Read selected configuration within path scope; secret rules still apply.",
            sets: &["implementer"],
        },
        BuiltinPermissionOperation {
            key: "workspace_config.commit",
            description: "Commit selected configuration within path scope and provider compare checks.",
            sets: &["implementer"],
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::super::failure::ConfigFailureOperation as Operation;
    use super::*;

    #[test]
    fn public_client_operations_use_declared_permissions() {
        for operation in [
            Operation::Current,
            Operation::Attach,
            Operation::Observe,
            Operation::Read,
            Operation::Commit,
        ] {
            assert!(
                DECLARATION
                    .operations
                    .iter()
                    .any(|declared| declared.key == operation.authorization()),
                "{operation:?}"
            );
        }
        assert_eq!(
            Operation::Current.authorization(),
            "workspace_config.observe"
        );
    }
}
