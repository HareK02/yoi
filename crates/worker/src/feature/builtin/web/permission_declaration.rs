//! Payload-free capabilities owned by this Feature, not by the legacy resource editor.

use crate::feature::permissions::{BuiltinPermissionDeclaration, BuiltinPermissionOperation};

pub(crate) const DECLARATION: BuiltinPermissionDeclaration = BuiltinPermissionDeclaration {
    feature: "web",
    namespace: "web",
    operations: &[
        BuiltinPermissionOperation {
            key: "web.fetch",
            description: "Fetch this exact HTTP(S) URL. Each redirect needs its own permission; private/local hosts remain denied.",
            sets: &["implementer", "companion", "work-execution"],
        },
        BuiltinPermissionOperation {
            key: "web.search",
            description: "Contact this exact search provider endpoint; credentials and network restrictions still apply.",
            sets: &["implementer", "companion", "work-execution"],
        },
    ],
};
