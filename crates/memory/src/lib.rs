//! Shared Memory domain and Workspace API transport types.
//!
//! Normal Memory persistence is owned by the Workspace control plane. This crate
//! intentionally contains no repository-local `.yoi` layout, discovery, reader,
//! writer, or fallback implementation.

pub mod audit;
pub mod backend;
pub mod extract;
pub mod schema;

pub use extract::ExtractPointerPayload;
pub use lint_common::{RecordLintError, Slug, is_valid_slug};

#[cfg(test)]
mod authority_tests {
    #[test]
    fn production_source_has_no_repository_local_memory_provider() {
        let production = [
            include_str!("lib.rs")
                .split_once("#[cfg(test)]\nmod authority_tests")
                .map(|(production, _)| production)
                .expect("Memory authority test module marker"),
            include_str!("backend.rs"),
            include_str!("audit.rs"),
            include_str!("extract/mod.rs"),
            include_str!("extract/input.rs"),
            include_str!("extract/payload.rs"),
            include_str!("extract/pointer.rs"),
            include_str!("extract/tool.rs"),
        ]
        .join("\n");
        for forbidden in [
            "WorkspaceLayout",
            "execute_memory_backend_operation",
            "collect_resident_summary",
            "deny_write_rules",
            "write_staging_candidate",
            ".yoi/memory",
        ] {
            assert!(
                !production.contains(forbidden),
                "repository-local Memory provider returned through {forbidden}"
            );
        }
    }
}
