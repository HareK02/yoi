use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};

const SUBMIT_TOOL: &str = "SubmitMemorySurface";
const SUBMIT_DESCRIPTION: &str = "Submit a bounded Markdown Memory surface whose every point cites exact refs from the supplied confirmed-Memory materials. Reference validation proves existence and scope, not semantic correctness.";

#[derive(Clone)]
pub(crate) struct MemorySurfaceOutputState {
    materials: Arc<Vec<server_api::SubjektivSurfaceMaterial>>,
    body_token_budget: usize,
    submitted: Arc<Mutex<Option<Vec<server_api::SubjektivSurfacePoint>>>>,
}

impl MemorySurfaceOutputState {
    pub(crate) fn new(
        materials: Vec<server_api::SubjektivSurfaceMaterial>,
        body_token_budget: usize,
    ) -> Self {
        Self {
            materials: Arc::new(materials),
            body_token_budget,
            submitted: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn submitted(&self) -> Option<Vec<server_api::SubjektivSurfacePoint>> {
        self.submitted
            .lock()
            .expect("Memory surface output state poisoned")
            .clone()
    }

    fn validate(&self, points: &[server_api::SubjektivSurfacePoint]) -> Result<(), ToolError> {
        if self.materials.is_empty() {
            return if points.is_empty() {
                Ok(())
            } else {
                Err(ToolError::InvalidArgument(
                    "empty materials require an empty points array".into(),
                ))
            };
        }
        if points.is_empty() {
            return Err(ToolError::InvalidArgument(
                "non-empty materials require at least one grounded point".into(),
            ));
        }
        if points.len() > 24 {
            return Err(ToolError::InvalidArgument(format!(
                "points count {} exceeds the configured limit 24",
                points.len()
            )));
        }
        let allowed = self
            .materials
            .iter()
            .map(|material| (material.memory_id.as_str(), material.revision))
            .collect::<HashSet<_>>();
        let mut bodies = Vec::with_capacity(points.len());
        for point in points {
            let body = point.body_md.trim();
            if body.is_empty() {
                return Err(ToolError::InvalidArgument(
                    "surface point body_md must not be empty".into(),
                ));
            }
            if point.memory_refs.is_empty() {
                return Err(ToolError::InvalidArgument(
                    "every surface point requires at least one memory_ref".into(),
                ));
            }
            let mut point_refs = HashSet::new();
            for reference in &point.memory_refs {
                if !point_refs.insert((reference.memory_id.as_str(), reference.revision)) {
                    return Err(ToolError::InvalidArgument(format!(
                        "duplicate surface point ref {}@{}",
                        reference.memory_id, reference.revision
                    )));
                }
                if !allowed.contains(&(reference.memory_id.as_str(), reference.revision)) {
                    return Err(ToolError::InvalidArgument(format!(
                        "surface ref {}@{} is not in the generation materials",
                        reference.memory_id, reference.revision
                    )));
                }
            }
            bodies.push(body);
        }
        let bytes = bodies
            .iter()
            .map(|body| body.len())
            .sum::<usize>()
            .saturating_add(points.len().saturating_sub(1) * 2);
        let tokens = bytes.saturating_add(3) / 4;
        if tokens > self.body_token_budget {
            return Err(ToolError::InvalidArgument(format!(
                "surface body token estimate {tokens} exceeds budget {}",
                self.body_token_budget
            )));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct MemorySurfaceOutputFeature {
    state: MemorySurfaceOutputState,
}

impl MemorySurfaceOutputFeature {
    pub(crate) fn new(state: MemorySurfaceOutputState) -> Self {
        Self { state }
    }
}

impl FeatureModule for MemorySurfaceOutputFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("memory-surface-output", "Memory Surface Output")
            .with_description(
                "Restricted structured output for one clean-context Memory surface editor.",
            )
            .with_tool(ToolDeclaration::new(SUBMIT_TOOL, SUBMIT_DESCRIPTION))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.tools().register(ToolContribution::new(
            SUBMIT_TOOL,
            submit_definition(self.state.clone()),
        ))
    }
}

fn submit_definition(state: MemorySurfaceOutputState) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(SubmitMemorySurfaceParams))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new(SUBMIT_TOOL)
            .description(SUBMIT_DESCRIPTION)
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(SubmitMemorySurfaceTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SubmitMemorySurfaceParams {
    points: Vec<server_api::SubjektivSurfacePoint>,
}

struct SubmitMemorySurfaceTool {
    state: MemorySurfaceOutputState,
}

#[async_trait]
impl Tool for SubmitMemorySurfaceTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: SubmitMemorySurfaceParams =
            serde_json::from_str(input_json).map_err(|error| {
                ToolError::InvalidArgument(format!("invalid SubmitMemorySurface input: {error}"))
            })?;
        self.state.validate(&params.points)?;
        let mut submitted = self
            .state
            .submitted
            .lock()
            .expect("Memory surface output state poisoned");
        if submitted.is_some() {
            return Err(ToolError::InvalidArgument(
                "SubmitMemorySurface already accepted output for this generation".into(),
            ));
        }
        let count = params.points.len();
        *submitted = Some(params.points);
        Ok(ToolOutput {
            summary: format!("Accepted {count} grounded Memory surface point(s)."),
            content: None,
            attachments: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory::extract::CandidateKind;

    fn material(id: &str) -> server_api::SubjektivSurfaceMaterial {
        server_api::SubjektivSurfaceMaterial {
            memory_id: id.into(),
            revision: 1,
            kind: CandidateKind::Constraint,
            body_md: "Keep the boundary.".into(),
            why_useful: "Avoid regression.".into(),
            staleness: Some("Until the boundary changes.".into()),
        }
    }

    fn point(id: &str) -> server_api::SubjektivSurfacePoint {
        server_api::SubjektivSurfacePoint {
            body_md: "- Keep the boundary unless it changes.".into(),
            memory_refs: vec![server_api::SubjektivMemoryRevisionRef {
                memory_id: id.into(),
                revision: 1,
            }],
        }
    }

    #[test]
    fn rejects_missing_or_material_external_refs_and_budget_overflow() {
        let state = MemorySurfaceOutputState::new(vec![material("allowed")], 32);
        assert!(state.validate(&[]).is_err());
        assert!(state.validate(&[point("other")]).is_err());
        let mut oversized = point("allowed");
        oversized.body_md = "x".repeat(132);
        assert!(state.validate(&[oversized]).is_err());
        assert!(state.validate(&[point("allowed")]).is_ok());
    }

    #[test]
    fn empty_materials_accept_only_normal_empty_output() {
        let state = MemorySurfaceOutputState::new(Vec::new(), 32);
        assert!(state.validate(&[]).is_ok());
        assert!(state.validate(&[point("invented")]).is_err());
    }
}
