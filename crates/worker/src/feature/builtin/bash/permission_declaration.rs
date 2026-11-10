//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "bash",
    namespace: "bash",
    operations: &[BuiltinPermissionOperation {
        key: "bash.execute",
        description: "Requires the whole Workdir: path '.' and recursive true. A shell is not confined to a subdirectory. Explicit provider command authority is also required.",
        sets: &["implementer", "work-execution"],
    }],
};
