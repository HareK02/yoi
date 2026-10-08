use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureInstructionContribution,
    FeatureInstructionDeclaration, FeatureInstructionId, FeatureModule, ToolContribution,
    ToolDeclaration, ToolDefinition,
};
use crate::permission::permission_action_for;
use crate::wip::{
    WipCallContext, WipDynamicItem, WipDynamicItemResolver, WipDynamicMount, WipMountError,
    WipMountRegistry, WipNamespaceRoute, WipOperationError, WipOperationHandler,
    WipOperationOutput, WipProjection, WipProjectionKind, json_to_wip, wip_to_json,
};
use crate::worker::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod};
use agen::tool::{Tool, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use manifest::{MergeRequestFeatureConfig, ToolPermissionAction, ToolPermissionConfig};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use wip_protocol::{
    Documentation, INTERFACE_FORMAT_V1, InterfaceDescriptor, Object, OperationDeclaration,
    ParameterDeclaration, ProtocolError, ProtocolErrorCode, ReturnDeclaration, TypeExpr,
    Value as WipValue,
};

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

const ALL_KINDS: [Kind; 6] = [
    Kind::Show,
    Kind::Open,
    Kind::Review,
    Kind::Readiness,
    Kind::Complete,
    Kind::CompleteTicket,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Show,
    Readiness,
    Open,
    Complete,
    CompleteTicket,
    Review,
}
#[derive(Clone)]
struct MergeRequestTool {
    client: Arc<dyn WorkspaceClient>,
    kind: Kind,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct MergeRequestInput {
    merge_request_id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct ShowMergeRequestInput {
    merge_request_id: String,
    /// Return thread events strictly after this sequence.
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    after: Option<u64>,
    /// Bounded thread page size. The Backend also enforces its authoritative cap.
    #[schemars(range(min = 1, max = 200))]
    limit: Option<usize>,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct OpenMergeRequestInput {
    ticket: String,
    repository_key: String,
    selector_from: String,
    selector_to: String,
    #[serde(default)]
    summary: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct CompleteMergeRequestInput {
    merge_request_id: String,
    operation_id: String,
    approval_event_id: String,
    target_ref_before: String,
    target_ref_after: String,
    strategy: MergeStrategyInput,
    resolution: MergeResolutionInput,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct CompleteTicketInput {
    ticket: String,
    operation_id: String,
    item_revision: String,
    merge_request_ids: Vec<String>,
    requirement_approval_event_id: String,
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
struct ReviewMergeRequestInput {
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
            Self::Complete | Self::CompleteTicket => config.complete,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Show => "ShowMergeRequest",
            Self::Readiness => "CheckMergeRequestReadiness",
            Self::Open => "OpenMergeRequest",
            Self::Complete => "CompleteMergeRequest",
            Self::CompleteTicket => "CompleteTicket",
            Self::Review => "ReviewMergeRequest",
        }
    }
    fn schema(self) -> serde_json::Value {
        match self {
            Self::Show => json!(schemars::schema_for!(ShowMergeRequestInput)),
            Self::Readiness => json!(schemars::schema_for!(MergeRequestInput)),
            Self::Open => json!(schemars::schema_for!(OpenMergeRequestInput)),
            Self::Complete => json!(schemars::schema_for!(CompleteMergeRequestInput)),
            Self::CompleteTicket => json!(schemars::schema_for!(CompleteTicketInput)),
            Self::Review => json!(schemars::schema_for!(ReviewMergeRequestInput)),
        }
    }

    fn mutating(self) -> bool {
        matches!(
            self,
            Self::Open | Self::Review | Self::Complete | Self::CompleteTicket
        )
    }
}
#[async_trait]
impl Tool for MergeRequestTool {
    async fn execute(&self, input: &str, _: ToolExecutionContext) -> Result<ToolOutput, ToolError> {
        let ws = self.client.workspace_id().ok_or_else(|| {
            ToolError::ExecutionFailed("Merge Request tools require Workspace identity".into())
        })?;
        if matches!(self.kind, Kind::Show) {
            let value: ShowMergeRequestInput = parse(input)?;
            nonempty_named("merge_request_id", &value.merge_request_id)?;
            if value.limit.is_some_and(|limit| !(1..=200).contains(&limit)) {
                return Err(ToolError::InvalidArgument(
                    "limit must be between 1 and 200".into(),
                ));
            }
            return self.show_merge_request(ws, &value.merge_request_id, value.after, value.limit);
        }
        let (method, path, body) = match self.kind {
            Kind::Readiness => {
                let v: MergeRequestInput = parse(input)?;
                nonempty_named("merge_request_id", &v.merge_request_id)?;
                (
                    WorkspaceRequestMethod::Get,
                    format!(
                        "/api/w/{ws}/merge-requests/{}/readiness",
                        encode_path_segment(&v.merge_request_id)
                    ),
                    None,
                )
            }
            Kind::Show => unreachable!("ShowMergeRequest is handled above"),
            Kind::Open => {
                let v: OpenMergeRequestInput = parse(input)?;
                nonempty(&v.ticket)?;
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{ws}/tickets/{}/merge-request",
                        encode_path_segment(&v.ticket)
                    ),
                    Some(
                        json!({"repository_key":v.repository_key,"selector_from":v.selector_from,"selector_to":v.selector_to,"summary":v.summary}),
                    ),
                )
            }
            Kind::Complete => {
                let v: CompleteMergeRequestInput = parse(input)?;
                nonempty_named("merge_request_id", &v.merge_request_id)?;
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{ws}/merge-requests/{}/complete",
                        encode_path_segment(&v.merge_request_id)
                    ),
                    Some(
                        json!({"operation_id":v.operation_id,"approval_event_id":v.approval_event_id,"target_ref_before":v.target_ref_before,"target_ref_after":v.target_ref_after,"strategy":match v.strategy{MergeStrategyInput::FastForward=>"fast_forward",MergeStrategyInput::Merge=>"merge"},"resolution":match v.resolution{MergeResolutionInput::None=>"none",MergeResolutionInput::Clean=>"clean",MergeResolutionInput::ConflictsResolved=>"conflicts_resolved"}}),
                    ),
                )
            }
            Kind::CompleteTicket => {
                let v: CompleteTicketInput = parse(input)?;
                nonempty_named("ticket", &v.ticket)?;
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{ws}/tickets/{}/complete",
                        encode_path_segment(&v.ticket)
                    ),
                    Some(json!({
                        "operation_id": v.operation_id,
                        "item_revision": v.item_revision,
                        "merge_request_ids": v.merge_request_ids,
                        "requirement_approval_event_id": v.requirement_approval_event_id,
                    })),
                )
            }
            Kind::Review => {
                let v: ReviewMergeRequestInput = parse(input)?;
                let ctx = self.client.reviewer_context().ok_or_else(|| {
                    ToolError::ExecutionFailed(
                        "Review submit requires injected Reviewer capability".into(),
                    )
                })?;
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{ws}/merge-requests/{}/reviews",
                        encode_path_segment(&ctx.merge_request_id)
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
            return Err(api_error("Merge Request API", &res));
        }
        Ok(ToolOutput {
            summary: self.kind.name().into(),
            content: Some(res.body),
            attachments: vec![],
        })
    }
}

impl MergeRequestTool {
    fn show_merge_request(
        &self,
        workspace_id: &str,
        merge_request_id: &str,
        after: Option<u64>,
        limit: Option<usize>,
    ) -> Result<ToolOutput, ToolError> {
        let merge_request_id = encode_path_segment(merge_request_id);
        let mut path = format!("/api/w/{workspace_id}/merge-requests/{merge_request_id}");
        let mut query = Vec::new();
        if let Some(after) = after {
            query.push(format!("after={after}"));
        }
        if let Some(limit) = limit {
            query.push(format!("limit={limit}"));
        }
        if !query.is_empty() {
            path.push('?');
            path.push_str(&query.join("&"));
        }
        let response = self
            .client
            .execute(WorkspaceRequest::get(path))
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if !response.is_success() {
            return Err(api_error("Merge Request API", &response));
        }
        Ok(ToolOutput {
            summary: self.kind.name().into(),
            content: Some(response.body),
            attachments: vec![],
        })
    }
}

fn api_error(operation: &str, response: &crate::worker::WorkspaceResponse) -> ToolError {
    ToolError::ExecutionFailed(format!(
        "{operation} returned HTTP {}: {}",
        response.status,
        bounded_body(&response.body)
    ))
}

fn bounded_body(body: &str) -> String {
    const MAX_CHARS: usize = 4096;
    let mut chars = body.chars();
    let bounded: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{bounded}…")
    } else {
        bounded
    }
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn nonempty_named(name: &str, value: &str) -> Result<(), ToolError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        Err(ToolError::InvalidArgument(format!(
            "{name} must not be empty"
        )))
    } else {
        Ok(())
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

pub(crate) fn enabled_merge_request_definitions(
    client: Arc<dyn WorkspaceClient>,
    config: MergeRequestFeatureConfig,
) -> Vec<ToolDefinition> {
    ALL_KINDS
        .into_iter()
        .filter(|kind| kind.enabled(config))
        .map(|kind| definition(client.clone(), kind))
        .collect()
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
        for definition in enabled_merge_request_definitions(self.client.clone(), self.config) {
            let (meta, _) = definition();
            tools.register(ToolContribution::new(meta.name, definition))?;
        }
        Ok(())
    }
}

const MERGE_REQUEST_COLLECTION_INTERFACE: &str = "yoi.merge-request/collection/v1";
const MERGE_REQUEST_ITEM_INTERFACE: &str = "yoi.merge-request/item/v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeMergeRequestSurface {
    Collection,
    Item,
}

#[derive(Clone, Copy, Debug)]
struct NativeMergeRequestOperation {
    operation: &'static str,
    surface: NativeMergeRequestSurface,
    identity_field: Option<&'static str>,
}

fn native_merge_request_operation(kind: Kind) -> NativeMergeRequestOperation {
    match kind {
        Kind::Show => NativeMergeRequestOperation {
            operation: "read",
            surface: NativeMergeRequestSurface::Item,
            identity_field: Some("merge_request_id"),
        },
        Kind::Open => NativeMergeRequestOperation {
            operation: "open",
            surface: NativeMergeRequestSurface::Collection,
            identity_field: None,
        },
        Kind::Review => NativeMergeRequestOperation {
            operation: "review",
            surface: NativeMergeRequestSurface::Item,
            identity_field: None,
        },
        Kind::Readiness => NativeMergeRequestOperation {
            operation: "check_readiness",
            surface: NativeMergeRequestSurface::Item,
            identity_field: Some("merge_request_id"),
        },
        Kind::Complete => NativeMergeRequestOperation {
            operation: "complete",
            surface: NativeMergeRequestSurface::Item,
            identity_field: Some("merge_request_id"),
        },
        Kind::CompleteTicket => NativeMergeRequestOperation {
            operation: "complete_ticket",
            surface: NativeMergeRequestSurface::Collection,
            identity_field: None,
        },
    }
}

#[derive(Clone)]
struct NativeMergeRequestTool {
    name: String,
    schema: Value,
    tool: Arc<dyn Tool>,
    kind: Kind,
    projection: NativeMergeRequestOperation,
    description: String,
    client: Arc<dyn WorkspaceClient>,
}

fn native_merge_request_tools(
    client: Arc<dyn WorkspaceClient>,
    config: MergeRequestFeatureConfig,
) -> Result<Vec<NativeMergeRequestTool>, WipMountError> {
    ALL_KINDS
        .into_iter()
        .filter(|kind| kind.enabled(config))
        .map(|kind| {
            let (meta, tool) = definition(client.clone(), kind)();
            jsonschema::validator_for(&meta.input_schema).map_err(|error| {
                WipMountError::InvalidProjection {
                    route: "/merge-requests".into(),
                    message: format!("Merge Request tool schema cannot be retained: {error}"),
                }
            })?;
            Ok(NativeMergeRequestTool {
                name: meta.name,
                schema: meta.input_schema,
                tool,
                kind,
                projection: native_merge_request_operation(kind),
                description: meta.description,
                client: client.clone(),
            })
        })
        .collect()
}

#[derive(Default)]
struct MergeRequestRevisionState {
    revisions: HashMap<String, String>,
    mutation_sequence: u64,
}

type MergeRequestRevisions = Arc<Mutex<MergeRequestRevisionState>>;

/// Mount only the Merge Request operations enabled for this Worker as a native
/// collection plus route-bound item objects. The ordinary Tool definitions remain
/// unchanged for Tool mode and are claimed only from WIP compatibility projection.
pub fn mount_workspace_http_merge_request_wip(
    registry: &mut WipMountRegistry,
    client: Arc<dyn WorkspaceClient>,
    config: MergeRequestFeatureConfig,
    permissions: Option<ToolPermissionConfig>,
    namespace_route: &WipNamespaceRoute,
) -> Result<(), WipMountError> {
    let collection_route = namespace_route.root().to_string();
    let tools = native_merge_request_tools(client, config)?;
    let claimed_tools = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    let collection_tools = tools
        .iter()
        .filter(|tool| tool.projection.surface == NativeMergeRequestSurface::Collection)
        .cloned()
        .collect::<Vec<_>>();
    let item_tools = tools
        .iter()
        .filter(|tool| tool.projection.surface == NativeMergeRequestSurface::Item)
        .cloned()
        .collect::<Vec<_>>();
    let revisions = Arc::new(Mutex::new(MergeRequestRevisionState::default()));

    let collection_descriptor = merge_request_descriptor(
        &collection_tools,
        "Open Merge Requests and complete Tickets through existing scoped authority",
        "Only collection-scoped Merge Request Feature operations enabled for this Worker are published. Opening retains repository and selector inputs; Ticket completion retains its full guarded result-set inputs.",
    )?;
    registry.mount(WipProjection {
        route: collection_route.clone(),
        capability: "merge-request:collection".into(),
        kind: WipProjectionKind::Native,
        object: Object {
            name: "merge-requests".into(),
            description: Some(
                "Authoritative Merge Request collection through scoped Backend authority".into(),
            ),
            interfaces: vec![MERGE_REQUEST_COLLECTION_INTERFACE.into()],
            r#ref: Some("merge-request:collection".into()),
            validator: Some(merge_request_route_validator(
                &collection_route,
                "collection",
            )),
        },
        interface: MERGE_REQUEST_COLLECTION_INTERFACE.into(),
        interface_validator: Some(merge_request_descriptor_validator(&collection_descriptor)),
        descriptor: collection_descriptor,
        handler: Arc::new(MergeRequestCollectionWipHandler {
            tools: merge_request_operation_map(collection_tools),
            permissions: permissions.clone(),
            collection_route: collection_route.clone(),
            revisions: Arc::clone(&revisions),
        }),
    })?;

    let item_descriptor = merge_request_descriptor(
        &item_tools,
        "Inspect and operate on the Merge Request bound to this object route",
        "The subject Merge Request identity comes exclusively from the target route. Repository selectors, exact review capability/candidate, Ticket result snapshot, approval, target refs, strategy, and resolution remain ordinary Backend-validated preconditions.",
    )?;
    registry.mount_dynamic(WipDynamicMount {
        collection_route: collection_route.clone(),
        capability: "merge-request:item".into(),
        interface: MERGE_REQUEST_ITEM_INTERFACE.into(),
        interface_validator: Some(merge_request_descriptor_validator(&item_descriptor)),
        descriptor: item_descriptor,
        resolver: Arc::new(MergeRequestItemResolver {
            tools: merge_request_operation_map(item_tools),
            permissions,
            collection_route: collection_route.clone(),
            revisions,
        }),
    })?;
    registry.replace_compatibility_tools(&collection_route, claimed_tools)?;
    Ok(())
}

fn merge_request_operation_map(
    tools: Vec<NativeMergeRequestTool>,
) -> HashMap<String, NativeMergeRequestTool> {
    tools
        .into_iter()
        .map(|tool| (tool.projection.operation.to_string(), tool))
        .collect()
}

struct MergeRequestCollectionWipHandler {
    tools: HashMap<String, NativeMergeRequestTool>,
    permissions: Option<ToolPermissionConfig>,
    collection_route: String,
    revisions: MergeRequestRevisions,
}

#[async_trait]
impl WipOperationHandler for MergeRequestCollectionWipHandler {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, WipValue>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let tool = self
            .tools
            .get(operation)
            .ok_or_else(merge_request_operation_not_found)?;
        let input = native_merge_request_input(arguments, None, tool.projection.identity_field)?;
        let affected_merge_requests = if tool.kind == Kind::CompleteTicket {
            input
                .get("merge_request_ids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let output =
            execute_native_merge_request_tool(tool, &self.permissions, input, None, context)
                .await?;
        let mut response = merge_request_tool_output_json(output)?;
        match tool.kind {
            Kind::Open => {
                let reference = merge_request_reference(&response)
                    .filter(|reference| is_merge_request_route_reference(reference))
                    .ok_or_else(|| {
                        WipOperationError::OutcomeUnknown(
                            "OpenMergeRequest returned no routable Merge Request identity".into(),
                        )
                    })?
                    .to_string();
                record_merge_request_observation(&self.revisions, &reference, &response, true);
                if let Some(object) = response.as_object_mut() {
                    object.insert(
                        "path".into(),
                        Value::String(format!("{}/{}", self.collection_route, reference)),
                    );
                }
            }
            Kind::CompleteTicket => {
                for reference in affected_merge_requests {
                    if is_merge_request_route_reference(&reference) {
                        record_merge_request_observation(
                            &self.revisions,
                            &reference,
                            &response,
                            true,
                        );
                    }
                }
            }
            _ => {}
        }
        Ok(WipOperationOutput::native(
            json_to_wip(&response).map_err(WipOperationError::OutcomeUnknown)?,
        ))
    }
}

struct MergeRequestItemResolver {
    tools: HashMap<String, NativeMergeRequestTool>,
    permissions: Option<ToolPermissionConfig>,
    collection_route: String,
    revisions: MergeRequestRevisions,
}

impl WipDynamicItemResolver for MergeRequestItemResolver {
    fn resolve(&self, item_reference: &str) -> Option<WipDynamicItem> {
        if !is_merge_request_route_reference(item_reference) {
            return None;
        }
        let revision = self
            .revisions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .revisions
            .get(item_reference)
            .cloned()
            .unwrap_or_else(|| "unobserved".into());
        let route = format!("{}/{}", self.collection_route, item_reference);
        Some(WipDynamicItem {
            object: Object {
                name: item_reference.into(),
                description: Some("Authoritative Merge Request bound to this object route".into()),
                interfaces: vec![MERGE_REQUEST_ITEM_INTERFACE.into()],
                r#ref: Some(format!("merge-request:{item_reference}")),
                validator: Some(merge_request_route_validator(&route, &revision)),
            },
            handler: Arc::new(MergeRequestItemWipHandler {
                tools: self.tools.clone(),
                permissions: self.permissions.clone(),
                merge_request_id: item_reference.into(),
                revisions: Arc::clone(&self.revisions),
            }),
        })
    }
}

struct MergeRequestItemWipHandler {
    tools: HashMap<String, NativeMergeRequestTool>,
    permissions: Option<ToolPermissionConfig>,
    merge_request_id: String,
    revisions: MergeRequestRevisions,
}

#[async_trait]
impl WipOperationHandler for MergeRequestItemWipHandler {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, WipValue>,
        context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let tool = self
            .tools
            .get(operation)
            .ok_or_else(merge_request_operation_not_found)?;
        let input = native_merge_request_input(
            arguments,
            Some(&self.merge_request_id),
            tool.projection.identity_field,
        )?;
        let output = execute_native_merge_request_tool(
            tool,
            &self.permissions,
            input,
            Some(&self.merge_request_id),
            context,
        )
        .await?;
        let response = merge_request_tool_output_json(output)?;
        ensure_merge_request_response_identity(
            &response,
            &self.merge_request_id,
            tool.kind.mutating(),
        )?;
        record_merge_request_observation(
            &self.revisions,
            &self.merge_request_id,
            &response,
            tool.kind.mutating(),
        );
        Ok(WipOperationOutput::native(
            json_to_wip(&response).map_err(WipOperationError::OutcomeUnknown)?,
        ))
    }
}

async fn execute_native_merge_request_tool(
    tool: &NativeMergeRequestTool,
    permissions: &Option<ToolPermissionConfig>,
    input: Value,
    bound_merge_request: Option<&str>,
    context: WipCallContext,
) -> Result<ToolOutput, WipOperationError> {
    if tool.kind == Kind::Review {
        let reviewer = tool.client.reviewer_context().ok_or_else(|| {
            merge_request_protocol_failure(
                ProtocolErrorCode::PermissionDenied,
                "review requires the injected Reviewer capability",
            )
        })?;
        if Some(reviewer.merge_request_id.as_str()) != bound_merge_request {
            return Err(merge_request_protocol_failure(
                ProtocolErrorCode::PermissionDenied,
                "the object route does not match the injected Reviewer capability",
            ));
        }
    }
    let validator = jsonschema::validator_for(&tool.schema).map_err(|error| {
        merge_request_protocol_failure(
            ProtocolErrorCode::Internal,
            format!("stored Merge Request tool schema is invalid: {error}"),
        )
    })?;
    if let Err(error) = validator.validate(&input) {
        return Err(merge_request_protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("Merge Request operation input violates its typed Tool schema: {error}"),
        ));
    }
    authorize_native_merge_request(permissions, &tool.name, &input)?;
    let input = serde_json::to_string(&input).map_err(|error| {
        merge_request_protocol_failure(ProtocolErrorCode::InvalidArguments, error.to_string())
    })?;
    tool.tool
        .execute(&input, context.execution)
        .await
        .map_err(|error| map_merge_request_tool_error(error, tool.kind.mutating()))
}

fn native_merge_request_input(
    arguments: &BTreeMap<String, WipValue>,
    bound_merge_request: Option<&str>,
    identity_field: Option<&str>,
) -> Result<Value, WipOperationError> {
    if bound_merge_request.is_some() && arguments.contains_key("merge_request_id") {
        return Err(merge_request_protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            "the subject Merge Request is bound exclusively by the object route",
        ));
    }
    let mut input = serde_json::Map::new();
    for (name, value) in arguments {
        input.insert(
            name.clone(),
            wip_to_json(value).map_err(|message| {
                merge_request_protocol_failure(ProtocolErrorCode::InvalidArguments, message)
            })?,
        );
    }
    if let (Some(merge_request_id), Some(field)) = (bound_merge_request, identity_field) {
        input.insert(field.into(), Value::String(merge_request_id.into()));
    }
    Ok(Value::Object(input))
}

fn authorize_native_merge_request(
    permissions: &Option<ToolPermissionConfig>,
    tool_name: &str,
    input: &Value,
) -> Result<(), WipOperationError> {
    let Some(permissions) = permissions else {
        return Ok(());
    };
    match permission_action_for(permissions, tool_name, input) {
        ToolPermissionAction::Allow => Ok(()),
        ToolPermissionAction::Deny => Err(merge_request_protocol_failure(
            ProtocolErrorCode::PermissionDenied,
            format!("permission denied for projected tool `{tool_name}`"),
        )),
        ToolPermissionAction::Ask => Err(merge_request_protocol_failure(
            ProtocolErrorCode::PermissionDenied,
            format!(
                "permission approval is unavailable for projected tool `{tool_name}`; denied fail-closed"
            ),
        )),
    }
}

fn map_merge_request_tool_error(error: ToolError, _mutating: bool) -> WipOperationError {
    match error {
        ToolError::InvalidArgument(message) => {
            merge_request_protocol_failure(ProtocolErrorCode::InvalidArguments, message)
        }
        ToolError::Cancelled(output) => WipOperationError::Cancelled(output),
        ToolError::Interrupted(output) => WipOperationError::Interrupted(output),
        ToolError::StructuredConflict { code, message } => merge_request_protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("{code}: {message}"),
        ),
        ToolError::ExecutionFailed(message) => match merge_request_http_status(&message) {
            Some(401 | 403) => {
                merge_request_protocol_failure(ProtocolErrorCode::PermissionDenied, message)
            }
            Some(404) => merge_request_protocol_failure(ProtocolErrorCode::NotFound, message),
            Some(400 | 409 | 422) => {
                merge_request_protocol_failure(ProtocolErrorCode::InvalidArguments, message)
            }
            _ => WipOperationError::OutcomeUnknown(message),
        },
        ToolError::Internal(message) => WipOperationError::OutcomeUnknown(message),
    }
}

fn merge_request_http_status(message: &str) -> Option<u16> {
    let marker = "HTTP ";
    let start = message.find(marker)? + marker.len();
    message[start..]
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn merge_request_tool_output_json(output: ToolOutput) -> Result<Value, WipOperationError> {
    match output.content {
        Some(content) => serde_json::from_str(&content)
            .map_err(|error| WipOperationError::OutcomeUnknown(error.to_string())),
        None => Ok(json!({"summary": output.summary, "ok": true})),
    }
}

fn ensure_merge_request_response_identity(
    response: &Value,
    bound_reference: &str,
    mutating: bool,
) -> Result<(), WipOperationError> {
    if let Some(reference) = merge_request_reference(response)
        && reference != bound_reference
    {
        let message = format!(
            "Backend response Merge Request `{reference}` does not match route-bound `{bound_reference}`"
        );
        return if mutating {
            Err(WipOperationError::OutcomeUnknown(message))
        } else {
            Err(merge_request_protocol_failure(
                ProtocolErrorCode::Internal,
                message,
            ))
        };
    }
    Ok(())
}

fn merge_request_reference(response: &Value) -> Option<&str> {
    response.get("merge_request_id").and_then(Value::as_str)
}

fn record_merge_request_observation(
    revisions: &MergeRequestRevisions,
    reference: &str,
    response: &Value,
    mutation: bool,
) {
    let mut state = revisions.lock().unwrap_or_else(|error| error.into_inner());
    let revision = if mutation {
        state.mutation_sequence = state.mutation_sequence.saturating_add(1);
        format!(
            "{}#mutation-{}",
            merge_request_response_fingerprint(response),
            state.mutation_sequence
        )
    } else {
        merge_request_response_fingerprint(response)
    };
    state.revisions.insert(reference.to_string(), revision);
}

fn merge_request_response_fingerprint(response: &Value) -> String {
    let mut digest = Sha256::new();
    digest.update(response.to_string().as_bytes());
    let bytes = digest.finalize();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_merge_request_route_reference(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= 128
        && reference != "."
        && reference != ".."
        && reference
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
}

fn merge_request_route_validator(route: &str, revision: &str) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(route.as_bytes());
    digest.update([0]);
    digest.update(revision.as_bytes());
    digest.finalize().to_vec()
}

fn merge_request_descriptor_validator(descriptor: &InterfaceDescriptor) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(format!("{descriptor:?}").as_bytes());
    digest.finalize().to_vec()
}

fn merge_request_descriptor(
    tools: &[NativeMergeRequestTool],
    summary: &str,
    details: &str,
) -> Result<InterfaceDescriptor, WipMountError> {
    let operations = tools
        .iter()
        .map(merge_request_operation_declaration)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: Some(Documentation {
            summary: summary.into(),
            details: Some(details.into()),
        }),
        types: Vec::new(),
        operations,
    })
}

fn merge_request_operation_declaration(
    tool: &NativeMergeRequestTool,
) -> Result<OperationDeclaration, WipMountError> {
    let properties = tool
        .schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| WipMountError::InvalidProjection {
            route: "/merge-requests".into(),
            message: format!(
                "Merge Request tool `{}` schema has no properties",
                tool.name
            ),
        })?;
    let required = tool
        .schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let parameters = properties
        .iter()
        .filter(|(name, _)| tool.projection.identity_field != Some(name.as_str()))
        .map(|(name, schema)| ParameterDeclaration {
            name: name.clone(),
            required: required.contains(name.as_str()),
            documentation: schema
                .get("description")
                .and_then(Value::as_str)
                .map(|summary| Documentation {
                    summary: summary.into(),
                    details: None,
                }),
            r#type: merge_request_schema_type_expr(schema),
        })
        .collect();
    Ok(OperationDeclaration {
        name: tool.projection.operation.into(),
        documentation: Some(Documentation {
            summary: tool.description.clone(),
            details: Some(format!(
                "Delegates to the existing `{}` Merge Request operation and retains its exact JSON Schema and Backend authority checks.",
                tool.name
            )),
        }),
        parameters,
        returns: ReturnDeclaration {
            documentation: Some(Documentation {
                summary: "Authoritative bounded Merge Request operation result".into(),
                details: None,
            }),
            r#type: TypeExpr::Json,
        },
    })
}

fn merge_request_schema_type_expr(schema: &Value) -> TypeExpr {
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => TypeExpr::String,
        Some("boolean") => TypeExpr::Boolean,
        Some("integer") => TypeExpr::Integer,
        Some("number") => TypeExpr::Number,
        Some("array") => TypeExpr::List {
            items: Box::new(
                schema
                    .get("items")
                    .map(merge_request_schema_type_expr)
                    .unwrap_or(TypeExpr::Json),
            ),
        },
        _ => TypeExpr::Json,
    }
}

fn merge_request_protocol_failure(
    code: ProtocolErrorCode,
    message: impl Into<String>,
) -> WipOperationError {
    WipOperationError::Protocol(ProtocolError {
        code,
        message: message.into(),
    })
}

fn merge_request_operation_not_found() -> WipOperationError {
    merge_request_protocol_failure(
        ProtocolErrorCode::OperationNotFound,
        "operation is not published by this Merge Request interface",
    )
}

pub fn description(n: &str) -> Option<&'static str> {
    match n {
        "ShowMergeRequest" => Some(
            "Read the selector-based Merge Request, append-only thread, source-review freshness, and target-integration evidence before review, fix, or handoff decisions.",
        ),
        "CheckMergeRequestReadiness" => Some(
            "Resolve current provider refs and derive readiness from exact-source review evidence; source movement requires fresh review while target-only movement preserves unchanged-source approval.",
        ),
        "OpenMergeRequest" => Some(
            "Open one repository-scoped Merge Request linked to the Ticket; reuse an existing Merge Request for that repository and advance only its selector_from with a normal non-force push.",
        ),
        "CompleteMergeRequest" => Some(
            "Record Orchestrator-owned integration for the explicitly addressed Merge Request without completing the Ticket or releasing its assignment.",
        ),
        "CompleteTicket" => Some(
            "Complete the Ticket only after reviewing its current item revision and exact linked Merge Request result set; this ends unfinished work atomically while retaining responsibility and assignment history.",
        ),
        "ReviewMergeRequest" => Some(
            "Submit the injected Reviewer capability result for its captured exact source ref; source movement cancels it, while target-only movement does not.",
        ),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::FeatureRegistryBuilder;
    use crate::hook::HookRegistryBuilder;
    use crate::worker::{TestWorkspaceHttpClient, WorkspaceClientError, WorkspaceResponse};
    use std::{collections::VecDeque, sync::Mutex};

    #[derive(Debug)]
    struct RecordingWorkspaceClient {
        responses: Mutex<VecDeque<WorkspaceResponse>>,
        requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl RecordingWorkspaceClient {
        fn new(responses: Vec<WorkspaceResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl WorkspaceClient for RecordingWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("ws")
        }

        fn kind(&self) -> &str {
            "recording"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests.lock().expect("request lock").push(request);
            self.responses
                .lock()
                .expect("response lock")
                .pop_front()
                .ok_or_else(|| WorkspaceClientError::Request("missing test response".into()))
        }
    }

    fn response(body: serde_json::Value) -> WorkspaceResponse {
        WorkspaceResponse {
            status: 200,
            body: body.to_string(),
        }
    }

    #[test]
    fn model_facing_operations_use_only_verb_first_names() {
        for name in [
            "ShowMergeRequest",
            "CheckMergeRequestReadiness",
            "OpenMergeRequest",
            "CompleteMergeRequest",
            "CompleteTicket",
            "ReviewMergeRequest",
        ] {
            assert!(description(name).is_some(), "missing operation {name}");
        }
        for legacy in [
            "MergeRequestShow",
            "MergeRequestReadinessCheck",
            "MergeRequestOpen",
            "MergeRequestComplete",
            "MergeRequestReview",
        ] {
            assert!(
                description(legacy).is_none(),
                "legacy alias {legacy} must not remain registered"
            );
        }
    }

    #[tokio::test]
    async fn show_reads_explicit_merge_request_resource() {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![response(
            json!({"merge_request_id":"MR/1","state":"open"}),
        )]));
        let tool = MergeRequestTool {
            kind: Kind::Show,
            client: client.clone(),
        };

        let output = tool
            .execute(
                r#"{"merge_request_id":"MR/1"}"#,
                ToolExecutionContext::default(),
            )
            .await
            .expect("show should succeed");

        assert_eq!(
            output.content.as_deref(),
            Some(r#"{"merge_request_id":"MR/1","state":"open"}"#)
        );
        let requests = client.requests.lock().expect("request lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, WorkspaceRequestMethod::Get);
        assert_eq!(requests[0].path, "/api/w/ws/merge-requests/MR%2F1");
    }

    #[tokio::test]
    async fn show_forwards_bounded_thread_pagination() {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![response(json!({
            "merge_request_id": "MR-1",
            "thread": []
        }))]));
        let tool = MergeRequestTool {
            kind: Kind::Show,
            client: client.clone(),
        };
        tool.execute(
            r#"{"merge_request_id":"MR-1","after":42,"limit":200}"#,
            ToolExecutionContext::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            client.requests.lock().expect("request lock")[0].path,
            "/api/w/ws/merge-requests/MR-1?after=42&limit=200"
        );

        let rejected = tool
            .execute(
                r#"{"merge_request_id":"MR-1","limit":201}"#,
                ToolExecutionContext::default(),
            )
            .await
            .expect_err("unbounded page size must fail before dispatch");
        assert!(rejected.to_string().contains("between 1 and 200"));
        assert_eq!(client.requests.lock().expect("request lock").len(), 1);
    }

    #[tokio::test]
    async fn show_requires_explicit_merge_request_identity() {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![]));
        let tool = MergeRequestTool {
            kind: Kind::Show,
            client: client.clone(),
        };

        let error = tool
            .execute(r#"{"ticket":"T1"}"#, ToolExecutionContext::default())
            .await
            .expect_err("ticket-only lookup must fail");

        assert!(error.to_string().contains("merge_request_id"));
        assert!(client.requests.lock().expect("request lock").is_empty());
    }

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
        assert_eq!(tool_names(coder), ["ShowMergeRequest", "OpenMergeRequest"]);

        let reviewer = MergeRequestFeatureConfig {
            show: true,
            review: true,
            ..Default::default()
        };
        assert_eq!(
            tool_names(reviewer),
            ["ShowMergeRequest", "ReviewMergeRequest"]
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
                "ShowMergeRequest",
                "CheckMergeRequestReadiness",
                "CompleteMergeRequest",
                "CompleteTicket"
            ]
        );
        assert_eq!(install(coder).1, [FEATURE_PROMPT_REF]);
        let unspecified = install(MergeRequestFeatureConfig::default());
        assert!(unspecified.0.is_empty());
        assert!(unspecified.1.is_empty());
    }

    #[test]
    fn native_projection_matches_coder_reviewer_and_orchestrator_inventory() {
        let client: Arc<dyn WorkspaceClient> =
            Arc::new(TestWorkspaceHttpClient::new("workspace", "http://unused"));
        let coder = MergeRequestFeatureConfig {
            show: true,
            open: true,
            ..Default::default()
        };
        let reviewer = MergeRequestFeatureConfig {
            show: true,
            review: true,
            ..Default::default()
        };
        let orchestrator = MergeRequestFeatureConfig {
            show: true,
            readiness_check: true,
            complete: true,
            ..Default::default()
        };

        let operations = |config| {
            native_merge_request_tools(client.clone(), config)
                .unwrap()
                .into_iter()
                .map(|tool| (tool.projection.surface, tool.projection.operation))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            operations(coder),
            [
                (NativeMergeRequestSurface::Item, "read"),
                (NativeMergeRequestSurface::Collection, "open"),
            ]
        );
        assert_eq!(
            operations(reviewer),
            [
                (NativeMergeRequestSurface::Item, "read"),
                (NativeMergeRequestSurface::Item, "review"),
            ]
        );
        assert_eq!(
            operations(orchestrator),
            [
                (NativeMergeRequestSurface::Item, "read"),
                (NativeMergeRequestSurface::Item, "check_readiness"),
                (NativeMergeRequestSurface::Item, "complete"),
                (NativeMergeRequestSurface::Collection, "complete_ticket"),
            ]
        );

        let tools = native_merge_request_tools(client, orchestrator).unwrap();
        let item = merge_request_descriptor(
            &tools
                .iter()
                .filter(|tool| tool.projection.surface == NativeMergeRequestSurface::Item)
                .cloned()
                .collect::<Vec<_>>(),
            "item",
            "route-bound",
        )
        .unwrap();
        assert!(item.operations.iter().all(|operation| {
            operation
                .parameters
                .iter()
                .all(|parameter| parameter.name != "merge_request_id")
        }));
        let complete = item
            .operations
            .iter()
            .find(|operation| operation.name == "complete")
            .unwrap();
        for required in [
            "operation_id",
            "approval_event_id",
            "target_ref_before",
            "target_ref_after",
            "strategy",
            "resolution",
        ] {
            assert!(
                complete
                    .parameters
                    .iter()
                    .any(|parameter| parameter.name == required),
                "completion precondition `{required}` disappeared"
            );
        }
    }

    #[test]
    fn native_mount_uses_host_route_and_accepts_only_one_canonical_item_segment() {
        let mut registry = WipMountRegistry::new();
        let namespace_route = registry
            .allocate_namespace("merge-request", "merge-requests")
            .unwrap();
        mount_workspace_http_merge_request_wip(
            &mut registry,
            Arc::new(TestWorkspaceHttpClient::new("workspace", "http://unused")),
            MergeRequestFeatureConfig {
                show: true,
                review: true,
                ..Default::default()
            },
            None,
            &namespace_route,
        )
        .unwrap();
        assert_eq!(registry.routes().collect::<Vec<_>>(), ["/merge-requests"]);
        let resolver = MergeRequestItemResolver {
            tools: HashMap::new(),
            permissions: None,
            collection_route: "/merge-requests".into(),
            revisions: Arc::new(Mutex::new(MergeRequestRevisionState::default())),
        };
        assert!(
            resolver
                .resolve("0199a7a8-1234-7000-8000-123456789abc")
                .is_some()
        );
        assert!(resolver.resolve("MR-1").is_some());
        for invalid in ["", ".", "..", "MR/1", "MR%2F1", "MR 1"] {
            assert!(resolver.resolve(invalid).is_none(), "resolved `{invalid}`");
        }
    }

    #[test]
    fn native_input_preserves_null_omission_permissions_and_route_identity() {
        let arguments = BTreeMap::from([
            ("body".into(), WipValue::String("approved".into())),
            ("findings".into(), json_to_wip(&json!([])).unwrap()),
            ("optional".into(), WipValue::Unit),
        ]);
        let input = native_merge_request_input(&arguments, Some("MR-1"), Some("merge_request_id"))
            .unwrap_or_else(|_| panic!("route-bound input should project"));
        assert_eq!(
            input,
            json!({
                "merge_request_id": "MR-1",
                "body": "approved",
                "findings": [],
                "optional": null
            })
        );
        let omitted = native_merge_request_input(
            &BTreeMap::from([("body".into(), WipValue::String("approved".into()))]),
            Some("MR-1"),
            Some("merge_request_id"),
        )
        .unwrap_or_else(|_| panic!("omitted optional input should project"));
        assert_eq!(
            omitted,
            json!({"merge_request_id": "MR-1", "body": "approved"})
        );
        assert!(matches!(
            native_merge_request_input(
                &BTreeMap::from([("merge_request_id".into(), WipValue::String("MR-2".into()))]),
                Some("MR-1"),
                Some("merge_request_id")
            ),
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::InvalidArguments,
                ..
            }))
        ));

        let permissions = Some(ToolPermissionConfig {
            default_action: ToolPermissionAction::Allow,
            rules: vec![manifest::ToolPermissionRule {
                tool: "ShowMergeRequest".into(),
                pattern: serde_json::to_string(&omitted).unwrap(),
                action: ToolPermissionAction::Deny,
            }],
        });
        assert!(matches!(
            authorize_native_merge_request(&permissions, "ShowMergeRequest", &omitted),
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                ..
            }))
        ));
    }

    #[tokio::test]
    async fn native_review_route_must_match_injected_capability_without_exposing_token() {
        use crate::worker::{ReviewerChildWorkspaceClient, ReviewerContext};

        let inner = Arc::new(RecordingWorkspaceClient::new(vec![response(json!({
            "event_id": "review-1",
            "subject_ref": "source-1",
            "decision": "approve"
        }))]));
        let reviewer: Arc<dyn WorkspaceClient> = Arc::new(ReviewerChildWorkspaceClient::new(
            inner.clone(),
            ReviewerContext {
                ticket_id: "T-1".into(),
                merge_request_id: "MR-1".into(),
            },
            "secret-capability".into(),
        ));
        let tool = native_merge_request_tools(
            reviewer,
            MergeRequestFeatureConfig {
                review: true,
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .find(|tool| tool.kind == Kind::Review)
        .unwrap();
        let descriptor =
            merge_request_descriptor(&[tool.clone()], "review", "capability-bound").unwrap();
        let serialized = format!("{descriptor:?}");
        assert!(!serialized.contains("secret-capability"));
        assert!(!serialized.contains("capability_token"));

        let input = json!({"decision": "approve", "body": "", "findings": []});
        let rejected = execute_native_merge_request_tool(
            &tool,
            &None,
            input.clone(),
            Some("MR-2"),
            WipCallContext {
                execution: ToolExecutionContext::direct(),
                security_context: "reviewer".into(),
            },
        )
        .await;
        assert!(matches!(
            rejected,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                ..
            }))
        ));
        assert!(inner.requests.lock().expect("request lock").is_empty());

        execute_native_merge_request_tool(
            &tool,
            &None,
            input,
            Some("MR-1"),
            WipCallContext {
                execution: ToolExecutionContext::direct(),
                security_context: "reviewer".into(),
            },
        )
        .await
        .unwrap_or_else(|_| panic!("matching Reviewer capability should execute"));
        let requests = inner.requests.lock().expect("request lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/api/w/ws/merge-requests/MR-1/reviews");
        let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["capability_token"], "secret-capability");
    }

    #[test]
    fn successful_observations_and_mutations_stale_only_affected_merge_requests() {
        let revisions = Arc::new(Mutex::new(MergeRequestRevisionState::default()));
        let resolver = MergeRequestItemResolver {
            tools: HashMap::new(),
            permissions: None,
            collection_route: "/merge-requests".into(),
            revisions: Arc::clone(&revisions),
        };
        let first = resolver.resolve("MR-1").unwrap().object.validator;
        record_merge_request_observation(
            &revisions,
            "MR-1",
            &json!({"merge_request_id": "MR-1", "updated_at": "rev-1"}),
            false,
        );
        let observed = resolver.resolve("MR-1").unwrap().object.validator;
        assert_ne!(first, observed);
        let other = resolver.resolve("MR-2").unwrap().object.validator;
        record_merge_request_observation(
            &revisions,
            "MR-1",
            &json!({"event_id": "review-1"}),
            true,
        );
        assert_ne!(observed, resolver.resolve("MR-1").unwrap().object.validator);
        assert_eq!(other, resolver.resolve("MR-2").unwrap().object.validator);
    }

    #[test]
    fn native_errors_preserve_backend_rejections_and_unknown_post_dispatch_outcomes() {
        for (status, code) in [
            (401, ProtocolErrorCode::PermissionDenied),
            (403, ProtocolErrorCode::PermissionDenied),
            (404, ProtocolErrorCode::NotFound),
            (409, ProtocolErrorCode::InvalidArguments),
            (422, ProtocolErrorCode::InvalidArguments),
        ] {
            assert!(matches!(
                map_merge_request_tool_error(
                    ToolError::ExecutionFailed(format!(
                        "Merge Request API returned HTTP {status}: rejected"
                    )),
                    true
                ),
                WipOperationError::Protocol(ProtocolError { code: actual, .. }) if actual == code
            ));
        }
        assert!(matches!(
            map_merge_request_tool_error(
                ToolError::ExecutionFailed("transport disconnected after dispatch".into()),
                true
            ),
            WipOperationError::OutcomeUnknown(_)
        ));
        assert!(matches!(
            merge_request_tool_output_json(ToolOutput {
                summary: "OpenMergeRequest".into(),
                content: Some("not-json".into()),
                attachments: vec![],
            }),
            Err(WipOperationError::OutcomeUnknown(_))
        ));
        assert!(matches!(
            ensure_merge_request_response_identity(
                &json!({"merge_request_id": "MR-2"}),
                "MR-1",
                true
            ),
            Err(WipOperationError::OutcomeUnknown(_))
        ));
    }

    #[tokio::test]
    async fn completion_forwards_every_integration_precondition_unchanged() {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![response(json!({
            "event_id": "merge-1",
            "approved_source_ref": "source-1",
            "target_ref_before": "target-1",
            "target_ref_after": "result-1"
        }))]));
        let tool = MergeRequestTool {
            kind: Kind::Complete,
            client: client.clone(),
        };
        tool.execute(
            r#"{"merge_request_id":"MR-1","operation_id":"op-1","approval_event_id":"approval-1","target_ref_before":"target-1","target_ref_after":"result-1","strategy":"fast_forward","resolution":"clean"}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
        let requests = client.requests.lock().expect("request lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/api/w/ws/merge-requests/MR-1/complete");
        let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({
                "operation_id": "op-1",
                "approval_event_id": "approval-1",
                "target_ref_before": "target-1",
                "target_ref_after": "result-1",
                "strategy": "fast_forward",
                "resolution": "clean"
            })
        );
    }

    #[test]
    fn schemas_hide_revision_and_commit_authority() {
        let schemas = [
            schemars::schema_for!(OpenMergeRequestInput),
            schemars::schema_for!(CompleteMergeRequestInput),
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
