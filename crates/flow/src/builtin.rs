//! Read-only catalog of embedded Flow sources.
//!
//! Provides exact source bytes without copying them into the Workspace DB or
//! maintaining manual update counters. The Runtime pins the compiled digest,
//! so resource updates do not change existing instance definitions.

use crate::{CompiledFlowDefinition, FlowCompileError, compile_flow_source};

pub const CODER_REVIEW_FLOW_SLUG: &str = "coder-review";

const CODER_REVIEW_FLOW_SOURCE: &str = include_str!("../../../resources/flows/coder-review.dcdl");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinFlowSource {
    pub slug: &'static str,
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

            path: "builtin/flows/coder-review.dcdl",
            content: CODER_REVIEW_FLOW_SOURCE,
        }),
        _ => None,
    }
}

pub fn builtin_flow_sources() -> &'static [BuiltinFlowSource] {
    const SOURCES: &[BuiltinFlowSource] = &[BuiltinFlowSource {
        slug: CODER_REVIEW_FLOW_SLUG,

        path: "builtin/flows/coder-review.dcdl",
        content: CODER_REVIEW_FLOW_SOURCE,
    }];
    SOURCES
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConditionVerdict, FlowRuntimeState, FlowVerifierOutcome, ResolvedFlowSource,
        TransitionConditionResult,
    };

    fn recipe() -> FlowRuntimeState {
        let definition = builtin_flow_source(CODER_REVIEW_FLOW_SLUG)
            .unwrap()
            .compile()
            .unwrap();
        FlowRuntimeState::start(
            &ResolvedFlowSource {
                selector: "builtin:coder-review".parse().unwrap(),
                workspace_id: "fixture".into(),
                flow_id: "fixture".into(),
                content_digest: definition.content_digest.clone(),
                definition,
            },
            "coder",
        )
        .unwrap()
        .0
    }

    // Typed graph/restore tests, not natural-language model judgment or MR authority.
    fn step(state: &mut FlowRuntimeState, id: &str, target: Option<&str>) {
        let attempt = state
            .begin_or_recover_transition(id, "fixture evidence")
            .unwrap();
        let results = attempt
            .transitions
            .iter()
            .map(|t| TransitionConditionResult {
                transition_id: t.transition_id.clone(),
                verdict: if target.is_some_and(|v| t.target.as_str() == v) {
                    ConditionVerdict::Met
                } else {
                    ConditionVerdict::NotMet
                },
                rationale: "Scripted condition result; no domain state or permission is issued"
                    .into(),
            })
            .collect();
        state
            .resolve_active_transition(id, FlowVerifierOutcome::Completed { results })
            .unwrap();
        if let Some(target) = target {
            assert_eq!(state.instance.current_state.as_str(), target);
        }
    }

    #[test]
    fn coder_recipe_reviews_fixes_then_integrates_without_early_terminal_handoff() {
        let mut state = recipe();
        for (n, next) in [
            "implement",
            "review",
            "fix",
            "review",
            "integrate",
            "review",
            "integrate",
            "complete",
            "done",
        ]
        .iter()
        .enumerate()
        {
            assert_eq!(state.instance.status, crate::FlowInstanceStatus::Active);
            step(&mut state, &format!("step-{n}"), Some(next));
        }
        assert_eq!(state.instance.status, crate::FlowInstanceStatus::Completed);
    }

    #[test]
    fn coder_recipe_resumes_integration_and_waits_without_success_or_redundant_review() {
        let mut state = recipe();
        step(&mut state, "existing-mr", Some("integrate"));
        step(&mut state, "pending-publication", None);
        assert_eq!(state.instance.current_state.as_str(), "integrate");
        assert_eq!(state.instance.status, crate::FlowInstanceStatus::Active);
        let mut restored: FlowRuntimeState =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(restored, state);
        step(&mut restored, "confirmed-publication", Some("complete"));
        step(&mut restored, "reported", Some("done"));
        let mut canceled = recipe();
        step(&mut canceled, "canceled-mr", Some("canceled"));
        assert_ne!(canceled.instance.current_state.as_str(), "done");
    }

    #[test]
    fn restored_old_recipe_keeps_its_pinned_definition_instead_of_catalog_migration() {
        let definition = compile_flow_source(
            r#"{ schema_version = 1; name = "coder-review"; initial = "old";
            states = { old = { instructions = "Pinned old instructions"; terminal = true; }; }; }"#,
        )
        .unwrap();
        let old = FlowRuntimeState::start(
            &ResolvedFlowSource {
                selector: "builtin:coder-review".parse().unwrap(),
                workspace_id: "fixture".into(),
                flow_id: "fixture".into(),
                content_digest: definition.content_digest.clone(),
                definition,
            },
            "old-session",
        )
        .unwrap()
        .0;
        let restored: FlowRuntimeState =
            serde_json::from_value(serde_json::to_value(&old).unwrap()).unwrap();
        assert_eq!(restored, old);
        assert_ne!(
            restored.definition.content_digest,
            recipe().definition.content_digest
        );
        assert_eq!(restored.instance.current_state.as_str(), "old");
    }

    #[test]
    fn every_builtin_flow_compiles_and_matches_catalog_identity() {
        assert!(!builtin_flow_sources().is_empty());
        for source in builtin_flow_sources() {
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
        }
    }
}
