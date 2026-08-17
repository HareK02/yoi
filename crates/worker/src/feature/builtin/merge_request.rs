use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureInstructionContribution,
    FeatureInstructionDeclaration, FeatureInstructionId, FeatureModule, ToolContribution,
    ToolDeclaration, ToolDefinition,
};
use crate::worker::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod};
use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use manifest::MergeRequestFeatureConfig;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub const FEATURE_ID: &str = "merge_request";
const FEATURE_NAME: &str = "Merge Request tools";
const FEATURE_DESCRIPTION: &str =
    "Operation-specific Merge Request workflow tools over Workspace authority.";
const FEATURE_INSTRUCTION_ID: &str = "merge_request.workflow";
pub const FEATURE_PROMPT_REF: &str = "common.merge_request";

fn workflow_instruction() -> FeatureInstructionDeclaration {
    FeatureInstructionDeclaration::new(
        FeatureInstructionId::builtin(FEATURE_INSTRUCTION_ID),
        FEATURE_PROMPT_REF,
        "Operation-specific Merge Request workflow guidance",
    )
    .expect("static Merge Request workflow instruction declaration is valid")
}

const ALL_KINDS: [Kind; 5] = [
    Kind::Show,
    Kind::Open,
    Kind::Review,
    Kind::Readiness,
    Kind::Complete,
];

#[derive(Clone, Copy)]
enum Kind {
    Show,
    Readiness,
    Open,
    Complete,
    Review,
}
#[derive(Clone)]
struct MergeRequestTool {
    client: Arc<dyn WorkspaceClient>,
    kind: Kind,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct ShowInput {
    ticket: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct OpenInput {
    ticket: String,
    repository_id: String,
    selector_from: String,
    selector_to: String,
    #[serde(default)]
    summary: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct CompleteInput {
    ticket: String,
    operation_id: String,
    approval_event_id: String,
    target_ref_before: String,
    target_ref_after: String,
    strategy: MergeStrategyInput,
    resolution: MergeResolutionInput,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MergeStrategyInput {
    FastForward,
    Merge,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MergeResolutionInput {
    None,
    Clean,
    ConflictsResolved,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct ReviewInput {
    decision: ReviewDecisionInput,
    #[serde(default)]
    body: String,
    #[serde(default)]
    findings: Vec<ReviewFindingInput>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ReviewDecisionInput {
    Approve,
    RequestChanges,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct ReviewFindingInput {
    severity: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    body: String,
}
impl Kind {
    fn enabled(self, config: MergeRequestFeatureConfig) -> bool {
        match self {
            Self::Show => config.show,
            Self::Open => config.open,
            Self::Review => config.review,
            Self::Readiness => config.readiness_check,
            Self::Complete => config.complete,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Show => "MergeRequestShow",
            Self::Readiness => "MergeRequestReadinessCheck",
            Self::Open => "MergeRequestOpen",
            Self::Complete => "MergeRequestComplete",
            Self::Review => "MergeRequestReview",
        }
    }
    fn schema(self) -> serde_json::Value {
        match self {
            Self::Show | Self::Readiness => json!(schemars::schema_for!(ShowInput)),
            Self::Open => json!(schemars::schema_for!(OpenInput)),
            Self::Complete => json!(schemars::schema_for!(CompleteInput)),
            Self::Review => json!(schemars::schema_for!(ReviewInput)),
        }
    }
}
#[async_trait]
impl Tool for MergeRequestTool {
    async fn execute(&self, input: &str, _: ToolExecutionContext) -> Result<ToolOutput, ToolError> {
        let ws = self.client.workspace_id().ok_or_else(|| {
            ToolError::ExecutionFailed("Merge Request tools require Workspace identity".into())
        })?;
        let (method, path, body) = match self.kind {
            Kind::Show | Kind::Readiness => {
                let v: ShowInput = parse(input)?;
                nonempty(&v.ticket)?;
                (
                    WorkspaceRequestMethod::Get,
                    format!(
                        "/api/w/{ws}/tickets/{}/merge-request{}",
                        v.ticket,
                        if matches!(self.kind, Kind::Readiness) {
                            "/readiness"
                        } else {
                            ""
                        }
                    ),
                    None,
                )
            }
            Kind::Open => {
                let v: OpenInput = parse(input)?;
                nonempty(&v.ticket)?;
                (
                    WorkspaceRequestMethod::Post,
                    format!("/api/w/{ws}/tickets/{}/merge-request", v.ticket),
                    Some(
                        json!({"repository_id":v.repository_id,"selector_from":v.selector_from,"selector_to":v.selector_to,"summary":v.summary}),
                    ),
                )
            }
            Kind::Complete => {
                let v: CompleteInput = parse(input)?;
                nonempty(&v.ticket)?;
                (
                    WorkspaceRequestMethod::Post,
                    format!("/api/w/{ws}/tickets/{}/merge-request/complete", v.ticket),
                    Some(
                        json!({"operation_id":v.operation_id,"approval_event_id":v.approval_event_id,"target_ref_before":v.target_ref_before,"target_ref_after":v.target_ref_after,"strategy":match v.strategy{MergeStrategyInput::FastForward=>"fast_forward",MergeStrategyInput::Merge=>"merge"},"resolution":match v.resolution{MergeResolutionInput::None=>"none",MergeResolutionInput::Clean=>"clean",MergeResolutionInput::ConflictsResolved=>"conflicts_resolved"}}),
                    ),
                )
            }
            Kind::Review => {
                let v: ReviewInput = parse(input)?;
                let ctx = self.client.reviewer_context().ok_or_else(|| {
                    ToolError::ExecutionFailed(
                        "Review submit requires injected Reviewer capability".into(),
                    )
                })?;
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{ws}/tickets/{}/merge-request/reviews",
                        ctx.ticket_id
                    ),
                    Some(
                        json!({"decision":match v.decision{ReviewDecisionInput::Approve=>"approve",ReviewDecisionInput::RequestChanges=>"request_changes"},"body":v.body,"findings":v.findings.into_iter().map(|f|json!({"severity":f.severity,"code":f.code,"path":f.path,"line":f.line,"body":f.body})).collect::<Vec<_>>() }),
                    ),
                )
            }
        };
        let req = match body {
            Some(v) => WorkspaceRequest::json(method, path, v.to_string()),
            None => WorkspaceRequest::get(path),
        };
        let res = self
            .client
            .execute(req)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        if !res.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "Merge Request API returned HTTP {}: {}",
                res.status, res.body
            )));
        }
        Ok(ToolOutput {
            summary: self.kind.name().into(),
            content: Some(res.body),
            attachments: vec![],
        })
    }
}
fn parse<T: serde::de::DeserializeOwned>(v: &str) -> Result<T, ToolError> {
    serde_json::from_str(v).map_err(|e| ToolError::InvalidArgument(e.to_string()))
}
fn nonempty(v: &str) -> Result<(), ToolError> {
    if v.trim().is_empty() {
        Err(ToolError::InvalidArgument(
            "ticket must not be empty".into(),
        ))
    } else {
        Ok(())
    }
}
fn definition(client: Arc<dyn WorkspaceClient>, kind: Kind) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(kind.name())
                .description(description(kind.name()).unwrap_or("Merge Request operation."))
                .input_schema(kind.schema()),
            Arc::new(MergeRequestTool {
                client: client.clone(),
                kind,
            }) as Arc<dyn Tool>,
        )
    })
}
pub struct MergeRequestFeature {
    client: Arc<dyn WorkspaceClient>,
    config: MergeRequestFeatureConfig,
}

impl MergeRequestFeature {
    pub fn new(client: Arc<dyn WorkspaceClient>, config: MergeRequestFeatureConfig) -> Self {
        Self { client, config }
    }

    fn kinds(&self) -> impl Iterator<Item = Kind> + '_ {
        ALL_KINDS
            .into_iter()
            .filter(|kind| kind.enabled(self.config))
    }
}

impl FeatureModule for MergeRequestFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin(FEATURE_ID, FEATURE_NAME)
            .with_description(FEATURE_DESCRIPTION);
        if self.config.any() {
            descriptor = descriptor.with_instruction(workflow_instruction());
        }
        for kind in self.kinds() {
            descriptor = descriptor.with_tool(ToolDeclaration::new(
                kind.name(),
                description(kind.name()).unwrap_or("Merge Request operation."),
            ));
        }
        descriptor
    }

    fn install(&self, ctx: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        if self.config.any() {
            ctx.instructions()
                .register(FeatureInstructionContribution::new(workflow_instruction()))?;
        }
        let mut tools = ctx.tools();
        for kind in self.kinds() {
            let definition = definition(self.client.clone(), kind);
            tools.register(ToolContribution::new(kind.name(), definition))?;
        }
        Ok(())
    }
}

pub fn description(n: &str) -> Option<&'static str> {
    match n {
        "MergeRequestShow" => Some("Read the selector-based Merge Request and append-only thread."),
        "MergeRequestReadinessCheck" => {
            Some("Resolve current provider refs and derive readiness from valid review events.")
        }
        "MergeRequestOpen" => {
            Some("Open a Merge Request with immutable source and target selectors.")
        }
        "MergeRequestComplete" => {
            Some("Complete using an approved review event and final target-ref evidence.")
        }
        "MergeRequestReview" => {
            Some("Submit the injected Reviewer capability result for its captured subject ref.")
        }
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::FeatureRegistryBuilder;
    use crate::hook::HookRegistryBuilder;
    use crate::worker::TestWorkspaceHttpClient;

    fn install(config: MergeRequestFeatureConfig) -> (Vec<String>, Vec<String>) {
        let client: Arc<dyn WorkspaceClient> =
            Arc::new(TestWorkspaceHttpClient::new("workspace", "http://unused"));
        let mut pending_tools = Vec::new();
        let mut hook_builder = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(MergeRequestFeature::new(client, config))
            .install_into_pending(&mut pending_tools, &mut hook_builder);
        assert!(!report.has_errors(), "{}", report.error_message());
        (
            report.installed_tool_names(),
            report
                .installed_instruction_contributions()
                .into_iter()
                .map(|instruction| instruction.prompt_ref)
                .collect(),
        )
    }

    fn tool_names(config: MergeRequestFeatureConfig) -> Vec<String> {
        install(config).0
    }

    #[test]
    fn flags_define_the_exact_registered_tool_surface() {
        let coder = MergeRequestFeatureConfig {
            show: true,
            open: true,
            ..Default::default()
        };
        assert_eq!(tool_names(coder), ["MergeRequestShow", "MergeRequestOpen"]);

        let reviewer = MergeRequestFeatureConfig {
            show: true,
            review: true,
            ..Default::default()
        };
        assert_eq!(
            tool_names(reviewer),
            ["MergeRequestShow", "MergeRequestReview"]
        );

        let orchestrator = MergeRequestFeatureConfig {
            show: true,
            readiness_check: true,
            complete: true,
            ..Default::default()
        };
        assert_eq!(
            tool_names(orchestrator),
            [
                "MergeRequestShow",
                "MergeRequestReadinessCheck",
                "MergeRequestComplete"
            ]
        );
        assert_eq!(install(coder).1, [FEATURE_PROMPT_REF]);
        let unspecified = install(MergeRequestFeatureConfig::default());
        assert!(unspecified.0.is_empty());
        assert!(unspecified.1.is_empty());
    }

    #[test]
    fn schemas_hide_revision_and_commit_authority() {
        let schemas = [
            schemars::schema_for!(OpenInput),
            schemars::schema_for!(CompleteInput),
        ];
        for s in schemas {
            let j = serde_json::to_string(&s).unwrap();
            for banned in [
                "revision_id",
                "attempt_id",
                "base_commit",
                "head_commit",
                "source_commit",
                "result_commit",
            ] {
                assert!(!j.contains(banned), "{banned} in {j}")
            }
        }
    }
}
