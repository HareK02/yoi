use crate::{CompiledFlowDefinition, FlowCompileError, compile_flow_source};

pub const CODER_REVIEW_FLOW_SLUG: &str = "coder-review";

const CODER_REVIEW_FLOW_SOURCE: &str = include_str!("../../../resources/flows/coder-review.dcdl");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinFlowSource {
    pub slug: &'static str,
    /// Monotonic resource revision. Increment when built-in semantics change;
    /// Runtime also pins the compiled content digest.
    pub revision: u64,
    pub path: &'static str,
    pub content: &'static str,
}

impl BuiltinFlowSource {
    pub fn compile(self) -> Result<CompiledFlowDefinition, FlowCompileError> {
        compile_flow_source(self.content)
    }
}

pub fn builtin_flow_source(slug: &str) -> Option<BuiltinFlowSource> {
    match slug {
        CODER_REVIEW_FLOW_SLUG => Some(BuiltinFlowSource {
            slug: CODER_REVIEW_FLOW_SLUG,
            revision: 3,
            path: "builtin/flows/coder-review.dcdl",
            content: CODER_REVIEW_FLOW_SOURCE,
        }),
        _ => None,
    }
}

pub fn builtin_flow_sources() -> &'static [BuiltinFlowSource] {
    const SOURCES: &[BuiltinFlowSource] = &[BuiltinFlowSource {
        slug: CODER_REVIEW_FLOW_SLUG,
        revision: 3,
        path: "builtin/flows/coder-review.dcdl",
        content: CODER_REVIEW_FLOW_SOURCE,
    }];
    SOURCES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_flow_compiles_and_matches_catalog_identity() {
        assert!(!builtin_flow_sources().is_empty());
        for source in builtin_flow_sources() {
            assert!(
                source.revision > 0,
                "built-in Flow revision must be positive"
            );
            let definition = source.compile().unwrap_or_else(|error| {
                panic!(
                    "builtin Flow {} failed to compile: {:?}",
                    source.slug, error.diagnostics
                )
            });
            assert_eq!(definition.name, source.slug);
            let selected =
                builtin_flow_source(source.slug).expect("builtin Flow must be selectable");
            assert_eq!(selected.content, source.content);
            assert_eq!(selected.revision, source.revision);
        }
    }

    #[test]
    fn coder_review_preserves_branch_policy_and_hands_approval_to_orchestrator() {
        let source =
            builtin_flow_source(CODER_REVIEW_FLOW_SLUG).expect("coder-review Flow must exist");

        for required in [
            "detached HEAD",
            "work/<ticket-id>-<slug>",
            "explicitly authorized",
            "git add",
            "git commit",
            "Workdir is clean",
            "current head commit",
            "same Ticket work branch",
            "new revision",
        ] {
            assert!(
                source.content.contains(required),
                "coder-review Flow must preserve branch/commit policy token {required:?}"
            );
        }
        assert!(source.content.contains("hand off to the Orchestrator"));
        assert!(source.content.contains("Do not call MergeRequestComplete"));
        assert!(!source.content.contains("Call MergeRequestComplete with"));
    }
}
