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
            revision: 4,
            path: "builtin/flows/coder-review.dcdl",
            content: CODER_REVIEW_FLOW_SOURCE,
        }),
        _ => None,
    }
}

pub fn builtin_flow_sources() -> &'static [BuiltinFlowSource] {
    const SOURCES: &[BuiltinFlowSource] = &[BuiltinFlowSource {
        slug: CODER_REVIEW_FLOW_SLUG,
        revision: 4,
        path: "builtin/flows/coder-review.dcdl",
        content: CODER_REVIEW_FLOW_SOURCE,
    }];
    SOURCES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coder_review_flow_uses_current_selector_ref_review_contract() {
        let source = builtin_flow_source(CODER_REVIEW_FLOW_SLUG).expect("coder review Flow");
        for required in [
            "OpenMergeRequest",
            "ShowMergeRequest",
            "ReviewMergeRequest",
            "CompleteMergeRequest",
            "existing Merge Request `selector_from`",
            "Target-only movement does not invalidate",
        ] {
            assert!(source.content.contains(required), "missing {required}");
        }
        for stale in [
            "MergeRequestOpen",
            "MergeRequestShow",
            "MergeRequestReview",
            "MergeRequestComplete",
            "new immutable revision",
        ] {
            assert!(!source.content.contains(stale), "stale contract {stale}");
        }
    }

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
}
