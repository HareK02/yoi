//! Connects enabled Merge Request Tools to the Workspace API and native WIP.
//!
//! The collection opens a repository candidate linked to a Ticket Entry; an item is one Merge
//! Request whose identity is exclusively route-bound. Keeping these Objects separate prevents a
//! read/review/publication from selecting a different subject in arguments. Review attests a fixed
//! source; Backend publication independently checks current authority and target eligibility.
//! Approval wakes automatic publication; this adapter neither runs Git locally nor concludes Tickets.
//!
//! Native v2 contracts are defined in the child `native` module, independently of Tool JSON Schema.
//! Their declarations drive descriptor generation, input validation, and command-field conversion.
//! Tool mode parses its own DTOs. Both enter the shared typed command dispatcher, including Backend
//! operation authorization; native never calls `Tool::execute`. Only configured actions are mounted.
//! [`crate::wip`] owns shared reference, Client-lifetime, and unknown-outcome contracts.
//!
//! # Injected review capability
//!
//! Review routes must exactly match the MR in the Host-injected Reviewer context. Capability tokens
//! never appear in descriptors or model arguments. Candidate, Ticket snapshot, approval, repository,
//! and integration evidence retain regular Tool and Backend checks. This adapter neither performs
//! Git/provider integration nor owns Ticket conclusion decisions.
//!
//! # Bounded responses and subject validation
//!
//! Read returns the existing bounded thread page, keeping linked Tickets as references. Successful
//! item-read response hashes are observed; mutations discard observations. Receipts do not fabricate
//! complete representations or local mutation generations. Native results separate a typed subject
//! Entry and explicit related Entries from the bounded domain JSON; the private `result` module
//! owns that projection boundary.
//! Item responses are checked against the bound MR; subject mismatches after mutations are not
//! treated as unexecuted operations. Native omission and explicit nullable unions are distinct;
//! native input changes do not change ordinary Tool omission/null behavior.

pub(crate) mod permission_declaration;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureInstructionContribution,
    FeatureInstructionDeclaration, FeatureInstructionId, FeatureModule, ToolContribution,
    ToolDeclaration, ToolDefinition,
};
use crate::wip::{
    WipCallContext, WipDynamicItem, WipDynamicItemResolver, WipDynamicMount, WipMountError,
    WipMountRegistry, WipNamespaceRoute, WipOperationError, WipOperationHandler,
    WipOperationOutput, WipProjection, WipProjectionKind, json_to_wip,
};
use crate::worker::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod};
use agen::tool::{Tool, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use manifest::MergeRequestFeatureConfig;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use server_api::authorization::PermissionResource;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use wip_protocol::{
    Documentation, INTERFACE_FORMAT_V1, InterfaceDescriptor, Object, OperationDeclaration,
    ProtocolError, ProtocolErrorCode, Value as WipValue,
};

pub const FEATURE_ID: &str = "merge_request";
const FEATURE_NAME: &str = "Merge Request tools";
const FEATURE_DESCRIPTION: &str = "Optional repository MR workflow with independent review and evidence-bound automatic publication; not a universal Ticket conclusion gate.";
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
    Kind::Query,
    Kind::Show,
    Kind::Open,
    Kind::Review,
    Kind::WorkerSet,
    Kind::Comment,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Query,
    Show,
    Open,
    Review,
    WorkerSet,
    Comment,
}
#[derive(Clone)]
struct MergeRequestTool {
    client: Arc<dyn WorkspaceClient>,
    kind: Kind,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMergeRequestWorkerInput {
    merge_request_id: String,
    #[serde(flatten)]
    request: server_api::SetMergeRequestWorkerRequest,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct CommentMergeRequestInput {
    merge_request_id: String,
    operation_id: String,
    expected_owner_operation_id: String,
    body: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct ShowMergeRequestInput {
    merge_request_id: String,
    /// summary (default) omits review bodies/history; detail returns exact paged events.
    /// For compatibility, supplying after/limit without view selects detail.
    view: Option<MergeRequestReadView>,
    /// Return thread events strictly after this sequence.
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    after: Option<u64>,
    /// Exact detail/history page size (default 20, maximum 200). Continue with event_page.next_after.
    #[schemars(range(min = 1, max = 200))]
    limit: Option<usize>,
}
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MergeRequestReadView {
    Summary,
    Detail,
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
            Self::Query => config.query,
            Self::Show => config.show,
            Self::Open => config.open,
            Self::Review => config.review,
            Self::WorkerSet => config.worker_set,
            Self::Comment => config.comment,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Query => "QueryMergeRequest",
            Self::Show => "ShowMergeRequest",
            Self::Open => "OpenMergeRequest",
            Self::Review => "ReviewMergeRequest",
            Self::WorkerSet => "SetMergeRequestWorker",
            Self::Comment => "CommentMergeRequest",
        }
    }
    fn schema(self) -> serde_json::Value {
        match self {
            Self::Query => json!(schemars::schema_for!(server_api::MergeRequestListQuery)),
            Self::Show => json!(schemars::schema_for!(ShowMergeRequestInput)),
            Self::WorkerSet => json!(schemars::schema_for!(SetMergeRequestWorkerInput)),
            Self::Comment => json!(schemars::schema_for!(CommentMergeRequestInput)),
            Self::Open => json!(schemars::schema_for!(OpenMergeRequestInput)),
            Self::Review => json!(schemars::schema_for!(ReviewMergeRequestInput)),
        }
    }

    fn mutating(self) -> bool {
        matches!(
            self,
            Self::Open | Self::Review | Self::WorkerSet | Self::Comment
        )
    }
}
mod command;
mod native;
mod result;

#[async_trait]
impl Tool for MergeRequestTool {
    async fn execute(&self, input: &str, _: ToolExecutionContext) -> Result<ToolOutput, ToolError> {
        command::execute(
            self.client.clone(),
            command::Command::from_tool(self.kind, input)?,
        )
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
    fn declare_permissions(
        &self,
        registrar: &mut crate::feature::permissions::FeaturePermissionRegistrar<'_>,
    ) -> Result<(), FeatureInstallError> {
        permission_declaration::DECLARATION.declare(registrar)
    }

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

const MERGE_REQUEST_COLLECTION_INTERFACE: &str = "yoi.merge-request/collection/v2";
const MERGE_REQUEST_ITEM_INTERFACE: &str = "yoi.merge-request/item/v2";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeMergeRequestSurface {
    Collection,
    Item,
}

#[derive(Clone, Copy, Debug)]
struct NativeMergeRequestOperation {
    surface: NativeMergeRequestSurface,
    identity_field: Option<&'static str>,
}

fn native_merge_request_operation(kind: Kind) -> NativeMergeRequestOperation {
    match kind {
        Kind::Show | Kind::WorkerSet | Kind::Comment => NativeMergeRequestOperation {
            surface: NativeMergeRequestSurface::Item,
            identity_field: Some("merge_request_id"),
        },
        Kind::Query | Kind::Open => NativeMergeRequestOperation {
            surface: NativeMergeRequestSurface::Collection,
            identity_field: None,
        },
        Kind::Review => NativeMergeRequestOperation {
            surface: NativeMergeRequestSurface::Item,
            identity_field: None,
        },
    }
}

#[derive(Clone)]
struct NativeMergeRequestTool {
    name: String,
    contract: native::Contract,
    roots: native::ReferenceRoots,
    kind: Kind,
    projection: NativeMergeRequestOperation,
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
            Ok(NativeMergeRequestTool {
                name: kind.name().into(),
                contract: native::contract(kind),
                roots: native::ReferenceRoots::default(),
                kind,
                projection: native_merge_request_operation(kind),
                client: client.clone(),
            })
        })
        .collect()
}

#[derive(Default)]
struct MergeRequestObservationState {
    content_digests: HashMap<String, String>,
}

type MergeRequestObservations = Arc<Mutex<MergeRequestObservationState>>;

/// Mount only the Merge Request operations enabled for this Worker as a native
/// collection plus route-bound item objects. The ordinary Tool definitions remain
/// unchanged for Tool mode and are claimed only from WIP compatibility projection.
pub fn mount_workspace_http_merge_request_wip(
    registry: &mut WipMountRegistry,
    client: Arc<dyn WorkspaceClient>,
    config: MergeRequestFeatureConfig,
    namespace_route: &WipNamespaceRoute,
) -> Result<(), WipMountError> {
    let collection_route = namespace_route.root().to_string();
    let roots = native::reference_roots(registry);
    let mut tools = native_merge_request_tools(client, config)?;
    for tool in &mut tools {
        tool.roots = roots.clone();
    }
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
    let observations = Arc::new(Mutex::new(MergeRequestObservationState::default()));

    let collection_descriptor = merge_request_descriptor(
        &collection_tools,
        "Query and open Merge Requests through existing scoped authority",
        "Open links a Ticket Entry to a repository {key, source, target}. Collection and route-bound item objects separate creation from subject operations. Inspect v2 contracts; v1 flattened inputs are not accepted. Ticket conclusion is independent.",
    )?;
    registry.mount(WipProjection {
        route: collection_route.clone(),
        capability: "merge-request:collection".into(),
        kind: WipProjectionKind::Native,
        object: Object {
            name: collection_route.trim_start_matches('/').into(),
            description: Some(
                "Authoritative Merge Request collection through scoped Backend authority".into(),
            ),
            interfaces: vec![crate::wip::root_reference(
                MERGE_REQUEST_COLLECTION_INTERFACE,
            )],
            r#ref: Some("merge-request:collection".into()),
            validator: Some(merge_request_route_validator(
                &collection_route,
                "collection",
            )),
        },
        interface: crate::wip::root_reference(MERGE_REQUEST_COLLECTION_INTERFACE),
        interface_validator: Some(merge_request_descriptor_validator(&collection_descriptor)),
        descriptor: collection_descriptor,
        handler: Arc::new(MergeRequestCollectionWipHandler {
            tools: merge_request_operation_map(collection_tools),
            collection_route: collection_route.clone(),
            observations: Arc::clone(&observations),
        }),
    })?;

    let item_descriptor = merge_request_descriptor(
        &item_tools,
        "Inspect and operate on the Merge Request bound to this object route",
        "The subject Merge Request identity comes exclusively from the target route. Read uses an optional page record. Review requires the exact injected capability. Backend owns automatic publication and evidence-bound completion. Backend validates business preconditions. No operation retries unknown outcomes.",
    )?;
    registry.mount_dynamic(WipDynamicMount {
        collection_route: collection_route.clone(),
        capability: "merge-request:item".into(),
        interface: crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE),
        interface_validator: Some(merge_request_descriptor_validator(&item_descriptor)),
        descriptor: item_descriptor,
        resolver: Arc::new(MergeRequestItemResolver {
            tools: merge_request_operation_map(item_tools),
            collection_route: collection_route.clone(),
            observations,
        }),
    })?;
    registry.replace_compatibility_tools(&collection_route, claimed_tools)?;
    registry.publish_result_namespace(namespace_route)?;
    Ok(())
}

fn merge_request_operation_map(
    tools: Vec<NativeMergeRequestTool>,
) -> HashMap<String, NativeMergeRequestTool> {
    tools
        .into_iter()
        .map(|tool| (tool.contract.name.to_string(), tool))
        .collect()
}

struct MergeRequestCollectionWipHandler {
    tools: HashMap<String, NativeMergeRequestTool>,
    collection_route: String,
    observations: MergeRequestObservations,
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
        let namespaces = context.result_namespaces.clone();
        let response = execute_native_merge_request_tool(tool, arguments, None, context).await?;
        if tool.kind == Kind::Query {
            return Ok(WipOperationOutput::native(result::value(
                tool.kind,
                response,
                "",
                &self.collection_route,
                namespaces.root("ticket"),
            )?));
        }
        let reference = merge_request_reference(&response)
            .filter(|reference| is_merge_request_route_reference(reference))
            .ok_or_else(|| {
                WipOperationError::OutcomeUnknown(
                    "OpenMergeRequest returned no routable Merge Request identity".into(),
                )
            })?
            .to_string();
        record_merge_request_observation(&self.observations, &reference, &response, true);
        Ok(WipOperationOutput::native(result::value(
            tool.kind,
            response,
            &reference,
            &self.collection_route,
            namespaces.root("ticket"),
        )?))
    }
}

struct MergeRequestItemResolver {
    tools: HashMap<String, NativeMergeRequestTool>,
    collection_route: String,
    observations: MergeRequestObservations,
}

impl WipDynamicItemResolver for MergeRequestItemResolver {
    fn resolve(&self, item_reference: &str) -> Option<WipDynamicItem> {
        if !is_merge_request_route_reference(item_reference) {
            return None;
        }
        let content_digest = self
            .observations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .content_digests
            .get(item_reference)
            .cloned();
        let route = format!("{}/{}", self.collection_route, item_reference);
        Some(WipDynamicItem {
            object: Object {
                name: item_reference.into(),
                description: Some("Authoritative Merge Request bound to this object route".into()),
                interfaces: vec![crate::wip::root_reference(MERGE_REQUEST_ITEM_INTERFACE)],
                r#ref: Some(format!("merge-request:{item_reference}")),
                validator: content_digest
                    .as_deref()
                    .map(|digest| merge_request_route_validator(&route, digest)),
            },
            handler: Arc::new(MergeRequestItemWipHandler {
                tools: self.tools.clone(),
                merge_request_id: item_reference.into(),
                collection_route: self.collection_route.clone(),
                observations: Arc::clone(&self.observations),
            }),
        })
    }
}

struct MergeRequestItemWipHandler {
    tools: HashMap<String, NativeMergeRequestTool>,
    merge_request_id: String,
    collection_route: String,
    observations: MergeRequestObservations,
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
        let namespaces = context.result_namespaces.clone();
        let response = execute_native_merge_request_tool(
            tool,
            arguments,
            Some(&self.merge_request_id),
            context,
        )
        .await?;
        ensure_merge_request_response_identity(
            &response,
            &self.merge_request_id,
            tool.kind.mutating(),
        )?;
        record_merge_request_observation(
            &self.observations,
            &self.merge_request_id,
            &response,
            tool.kind.mutating(),
        );
        Ok(WipOperationOutput::native(result::value(
            tool.kind,
            response,
            &self.merge_request_id,
            &self.collection_route,
            namespaces.root("ticket"),
        )?))
    }
}

async fn execute_native_merge_request_tool(
    tool: &NativeMergeRequestTool,
    arguments: &BTreeMap<String, WipValue>,
    bound_merge_request: Option<&str>,
    _context: WipCallContext,
) -> Result<Value, WipOperationError> {
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
    let input = native_merge_request_input(tool, arguments, bound_merge_request)?;
    let command = native::command(tool.kind, input)?;
    command::execute_value(tool.client.clone(), command)
        .map_err(|error| map_merge_request_tool_error(error, tool.kind.mutating()))
}

fn native_merge_request_input(
    tool: &NativeMergeRequestTool,
    arguments: &BTreeMap<String, WipValue>,
    bound_merge_request: Option<&str>,
) -> Result<Value, WipOperationError> {
    let mut input = native::convert(&tool.contract, arguments, &tool.roots)?;
    if let (Some(merge_request_id), Some(field)) =
        (bound_merge_request, tool.projection.identity_field)
    {
        input
            .as_object_mut()
            .expect("native contract produces a record")
            .insert(field.into(), Value::String(merge_request_id.into()));
    }
    Ok(input)
}

fn map_merge_request_tool_error(error: ToolError, _mutating: bool) -> WipOperationError {
    match error {
        ToolError::InvalidArgument(message) => {
            merge_request_protocol_failure(ProtocolErrorCode::InvalidArguments, message)
        }
        ToolError::ExecutionFailure(output) => WipOperationError::ExecutionFailure(output),
        ToolError::Cancelled(output) => WipOperationError::Cancelled(output),
        ToolError::Interrupted(output) => WipOperationError::Interrupted(output),
        ToolError::StructuredConflict { code, message } => merge_request_protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("{code}: {message}"),
        ),
        ToolError::ExecutionFailed(message)
            if message
                .starts_with("Merge Request operation authorization failed before dispatch:") =>
        {
            merge_request_protocol_failure(
                if message.ends_with("denied=true") {
                    ProtocolErrorCode::PermissionDenied
                } else {
                    ProtocolErrorCode::Internal
                },
                message,
            )
        }
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
    observations: &MergeRequestObservations,
    reference: &str,
    response: &Value,
    mutation: bool,
) {
    let mut state = observations
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if mutation {
        state.content_digests.remove(reference);
    } else {
        state.content_digests.insert(
            reference.to_string(),
            merge_request_response_fingerprint(response),
        );
    }
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

fn merge_request_route_validator(route: &str, content_digest: &str) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(route.as_bytes());
    digest.update([0]);
    digest.update(content_digest.as_bytes());
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
    Ok(native::declaration(&tool.contract, &tool.roots))
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
        "QueryMergeRequest" => Some(
            "Query a bounded MR collection page by repository_key, selector_to (branch shorthand or refs/heads/...), exact phase, legacy state, linked ticket_ref and source selector. phase=integration includes stale reviews, conflicts and waiting MRs, not only currently approved candidates. assigned_to_self=true matches the authenticated current MR Worker, not the Ticket assigned Subject or historical owners. Omitted/false means no assignment filter. Continue with next_cursor and unchanged filters; null next_cursor ends the listing. Query permission does not grant individual MR reads. Use ShowMergeRequest on selected results before acting. Does not assign, publish, approve, or establish execution order.",
        ),
        "ShowMergeRequest" => Some(
            "Read an authoritative compact MR summary by default: source/target observations, full-evidence review status/readiness and stable event references, worker_assignment (operation_id and worker), and saved publications, without review bodies/history. Both summary and detail include worker_assignment and publications. worker_assignment=null means never assigned; worker=null in a retained assignment is a release that retains its operation_id. Ownership is not resource authority, Worker liveness, or Ticket assignment. Resume implementation or integration from the saved MR phase, current worker, source, review and publication reasons rather than a prior Session or Flow state. Publication outcomes are evidence only; this read never sends or retries publication. Pending/unknown outcomes are not failure; yield without polling and reconcile before source mutation or a new push. Use view=detail with bounded after/limit for exact reviews/findings and history; continue with event_page.next_after. Status is never inferred from a history page. Summary is not approval or integration authority.",
        ),
        "SetMergeRequestWorker" => Some(
            "Set, replace, or release this MR's Worker after reading ShowMergeRequest view=summary and its worker_assignment. Use worker_assignment.operation_id as expected_operation_id; worker_assignment=null means never assigned, while worker=null in a retained assignment is a release with a retained operation_id. Supply a stable operation_id, required expected_operation_id (explicit null only when never assigned), and required worker {runtime_id,worker_id} or explicit null to release. Exact retries retain the same key and full input. On conflict reread and reconcile; unknown outcomes require reconciliation, never a new key or automatic retry. Does not reassign the Ticket, grant authority, start/stop Workers, change MR phase or approval, or publish code.",
        ),
        "CommentMergeRequest" => Some(
            "Append a Markdown comment to the MR thread as its current Worker. Use the observed Worker assignment operation_id as expected_owner_operation_id and a stable comment operation_id. Replays must retain the same full input; stale-owner writes fail. Record conflicts, questions and resolutions here without manufacturing approval or publication evidence. No state change, Git operation or Ticket conclusion.",
        ),
        "OpenMergeRequest" => Some(
            "Open one repository-scoped Merge Request linked to the Ticket; reuse an existing Merge Request for that repository and advance only its selector_from with a normal non-force push.",
        ),
        "ReviewMergeRequest" => Some(
            "Submit the injected trusted Reviewer capability result for its captured exact source ref; source movement cancels it, while target-only movement does not. Approval triggers Backend publication when operation permissions and target conditions allow it, without another Subject stamp or model turn. The capability authenticates the attempt, not the verdict: independent natural-language judgment and validation remain the Reviewer's responsibility.",
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
    pub(super) struct RecordingWorkspaceClient {
        pub(super) auth_denied: std::sync::atomic::AtomicBool,
        pub(super) authorizations:
            Mutex<Vec<server_api::authorization::OperationAuthorizationRequest>>,
        responses: Mutex<VecDeque<WorkspaceResponse>>,
        pub(super) requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl RecordingWorkspaceClient {
        pub(super) fn new(responses: Vec<WorkspaceResponse>) -> Self {
            Self {
                auth_denied: std::sync::atomic::AtomicBool::new(false),
                authorizations: Mutex::new(Vec::new()),
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
            if request.path.ends_with("/authorization/check") {
                let input: server_api::authorization::OperationAuthorizationRequest =
                    serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                self.authorizations.lock().unwrap().push(input.clone());
                if self.auth_denied.load(std::sync::atomic::Ordering::SeqCst) {
                    return Ok(WorkspaceResponse {
                        content_type: None,
                        status: 403,
                        body: "{}".into(),
                    });
                }
                return Ok(WorkspaceResponse {
                    content_type: None,
                    status: 200,
                    body: serde_json::json!({"scope": input.resource}).to_string(),
                });
            }
            self.requests.lock().expect("request lock").push(request);
            self.responses
                .lock()
                .expect("response lock")
                .pop_front()
                .ok_or_else(|| WorkspaceClientError::Request("missing test response".into()))
        }
    }

    pub(super) fn response(body: serde_json::Value) -> WorkspaceResponse {
        WorkspaceResponse {
            content_type: None,
            status: 200,
            body: body.to_string(),
        }
    }

    #[test]
    fn model_facing_operations_use_only_verb_first_names() {
        for name in ["ShowMergeRequest", "OpenMergeRequest", "ReviewMergeRequest"] {
            assert!(description(name).is_some(), "missing operation {name}");
        }
        for legacy in ["MergeRequestShow", "MergeRequestOpen", "MergeRequestReview"] {
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
        assert_eq!(
            requests[0].path,
            "/api/w/ws/merge-requests/MR%2F1?view=summary"
        );
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
            "/api/w/ws/merge-requests/MR-1?view=detail&after=42&limit=200"
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
    async fn native_read_forwards_explicit_projection_and_rejects_ambiguous_summary_paging() {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![response(json!({
            "merge_request_id": "MR-1", "view": "detail", "thread": [],
            "summary": {"review_status": "approved"},
            "event_page": {"omitted": false, "has_more": true, "next_after": 9, "limit": 1}
        }))]));
        let tool = native_merge_request_tools(
            client.clone(),
            MergeRequestFeatureConfig {
                show: true,
                ..Default::default()
            },
        )
        .unwrap()
        .remove(0);
        let arguments = BTreeMap::from([
            ("view".into(), WipValue::String("detail".into())),
            ("page".into(), json_to_wip(&json!({"after":{"$case":"value","value":8},"limit":{"$case":"value","value":1}})).unwrap()),
        ]);
        let output = execute_native_merge_request_tool(
            &tool,
            &arguments,
            Some("MR-1"),
            WipCallContext {
                execution: ToolExecutionContext::direct(),
                security_context: "reader".into(),
                result_namespaces: Default::default(),
            },
        )
        .await
        .unwrap_or_else(|_| panic!("native detail read should succeed"));
        assert_eq!(output["summary"]["review_status"], "approved");
        assert_eq!(output["event_page"]["next_after"], 9);
        assert_eq!(
            client.requests.lock().unwrap()[0].path,
            "/api/w/ws/merge-requests/MR-1?view=detail&after=8&limit=1"
        );
        let rejected = MergeRequestTool {
            kind: Kind::Show,
            client: client.clone(),
        }
        .execute(
            r#"{"merge_request_id":"MR-1","view":"summary","limit":1}"#,
            ToolExecutionContext::default(),
        )
        .await
        .unwrap_err();
        assert!(rejected.to_string().contains("require view=detail"));
        assert_eq!(client.requests.lock().unwrap().len(), 1);
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
            ..Default::default()
        };
        assert_eq!(tool_names(orchestrator), ["ShowMergeRequest",]);
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
            ..Default::default()
        };

        let operations = |config| {
            native_merge_request_tools(client.clone(), config)
                .unwrap()
                .into_iter()
                .map(|tool| (tool.projection.surface, tool.contract.name))
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
            [(NativeMergeRequestSurface::Item, "read"),]
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
            &namespace_route,
        )
        .unwrap();
        assert_eq!(registry.routes().collect::<Vec<_>>(), ["/merge-requests"]);
        let resolver = MergeRequestItemResolver {
            tools: HashMap::new(),
            collection_route: "/merge-requests".into(),
            observations: Arc::new(Mutex::new(MergeRequestObservationState::default())),
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
    fn native_input_preserves_null_omission_and_route_identity() {
        let tool = native_merge_request_tools(
            Arc::new(TestWorkspaceHttpClient::new("workspace", "http://unused")),
            MergeRequestFeatureConfig {
                show: true,
                ..Default::default()
            },
        )
        .unwrap()
        .remove(0);
        let arguments = BTreeMap::from([(
            "page".into(),
            json_to_wip(&json!({"after": {"$case": "null"}})).unwrap(),
        )]);
        let input = native_merge_request_input(&tool, &arguments, Some("MR-1"))
            .unwrap_or_else(|_| panic!("valid native input"));
        assert_eq!(input, json!({"merge_request_id": "MR-1", "after": null}));
        let omitted = native_merge_request_input(&tool, &BTreeMap::new(), Some("MR-1"))
            .unwrap_or_else(|_| panic!("valid native input"));
        assert_eq!(omitted, json!({"merge_request_id": "MR-1"}));
        assert!(matches!(
            native_merge_request_input(
                &tool,
                &BTreeMap::from([("merge_request_id".into(), WipValue::String("MR-2".into()))]),
                Some("MR-1")
            ),
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::InvalidArguments,
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

        let WipValue::Record(input) =
            json_to_wip(&json!({"decision": "approve", "body": "", "findings": []})).unwrap()
        else {
            unreachable!()
        };
        let rejected = execute_native_merge_request_tool(
            &tool,
            &input,
            Some("MR-2"),
            WipCallContext {
                result_namespaces: Default::default(),
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
            &input,
            Some("MR-1"),
            WipCallContext {
                result_namespaces: Default::default(),
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
        let observations = Arc::new(Mutex::new(MergeRequestObservationState::default()));
        let resolver = MergeRequestItemResolver {
            tools: HashMap::new(),
            collection_route: "/merge-requests".into(),
            observations: Arc::clone(&observations),
        };
        let first = resolver.resolve("MR-1").unwrap().object.validator;
        record_merge_request_observation(
            &observations,
            "MR-1",
            &json!({"merge_request_id": "MR-1", "updated_at": "rev-1"}),
            false,
        );
        let observed = resolver.resolve("MR-1").unwrap().object.validator;
        assert_ne!(first, observed);
        let other = resolver.resolve("MR-2").unwrap().object.validator;
        record_merge_request_observation(
            &observations,
            "MR-1",
            &json!({"event_id": "review-1"}),
            true,
        );
        assert_ne!(observed, resolver.resolve("MR-1").unwrap().object.validator);
        assert_eq!(other, resolver.resolve("MR-2").unwrap().object.validator);
    }

    #[test]
    fn merge_request_observation_digest_survives_identical_reads_without_mutation_counters() {
        let observations = Arc::new(Mutex::new(MergeRequestObservationState::default()));
        let resolver = MergeRequestItemResolver {
            tools: HashMap::new(),
            collection_route: "/merge-requests".into(),
            observations: Arc::clone(&observations),
        };
        let response = json!({"merge_request_id": "MR-1", "state": "open", "thread": []});
        assert!(resolver.resolve("MR-1").unwrap().object.validator.is_none());
        record_merge_request_observation(&observations, "MR-1", &response, false);
        let observed = resolver.resolve("MR-1").unwrap().object.validator;
        assert!(observed.is_some());
        record_merge_request_observation(&observations, "MR-1", &response, false);
        assert_eq!(resolver.resolve("MR-1").unwrap().object.validator, observed);
        for _ in 0..2 {
            record_merge_request_observation(
                &observations,
                "MR-1",
                &json!({"event_id": "same-review"}),
                true,
            );
            assert!(resolver.resolve("MR-1").unwrap().object.validator.is_none());
        }
        record_merge_request_observation(&observations, "MR-1", &response, false);
        assert_eq!(resolver.resolve("MR-1").unwrap().object.validator, observed);
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
        let invalid_json = Arc::new(RecordingWorkspaceClient::new(vec![WorkspaceResponse {
            content_type: None,
            status: 200,
            body: "not-json".into(),
        }]));
        let error = command::execute_value(
            invalid_json.clone(),
            command::Command::Read {
                view: None,
                merge_request_id: "MR-1".into(),
                after: None,
                limit: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            map_merge_request_tool_error(error, false),
            WipOperationError::OutcomeUnknown(_)
        ));
        assert_eq!(invalid_json.requests.lock().unwrap().len(), 1);
        assert!(matches!(
            ensure_merge_request_response_identity(
                &json!({"merge_request_id": "MR-2"}),
                "MR-1",
                true
            ),
            Err(WipOperationError::OutcomeUnknown(_))
        ));
    }

    #[test]
    fn schemas_hide_content_digest_and_commit_authority() {
        let schemas = [schemars::schema_for!(OpenMergeRequestInput)];
        for s in schemas {
            let j = serde_json::to_string(&s).unwrap();
            for banned in [
                "content_digest_id",
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
    #[tokio::test]
    async fn denied_merge_request_operations_do_not_reach_provider() {
        for (kind, input, operation) in [
            (Kind::Show, json!({"merge_request_id":"MR-1"}), "read"),
            (
                Kind::Open,
                json!({"ticket":"T-1", "repository_key":"repo", "selector_from":"work", "selector_to":"develop"}),
                "open",
            ),
            (Kind::Review, json!({"decision":"approve"}), "review"),
        ] {
            let client = Arc::new(RecordingWorkspaceClient::new(vec![]));
            client
                .auth_denied
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let bound: Arc<dyn WorkspaceClient> = if kind == Kind::Review {
                Arc::new(crate::worker::ReviewerChildWorkspaceClient::new(
                    client.clone(),
                    crate::worker::ReviewerContext {
                        ticket_id: "T-1".into(),
                        merge_request_id: "MR-1".into(),
                    },
                    "secret".into(),
                ))
            } else {
                client.clone()
            };
            let tool = MergeRequestTool {
                client: bound,
                kind,
            };
            let error = tool
                .execute(&input.to_string(), ToolExecutionContext::direct())
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("denied=true"),
                "{operation}: {error}"
            );
            assert!(matches!(
                map_merge_request_tool_error(error, kind.mutating()),
                WipOperationError::Protocol(ProtocolError {
                    code: ProtocolErrorCode::PermissionDenied,
                    ..
                })
            ));
            assert!(client.requests.lock().unwrap().is_empty());
            let auth = client.authorizations.lock().unwrap();
            assert_eq!(auth.len(), 1);
            assert_eq!(auth[0].operation, format!("merge_request.{operation}"));
            assert_eq!(
                auth[0].resource,
                if kind == Kind::Open {
                    PermissionResource::Ticket {
                        workspace_id: "ws".into(),
                        ticket_id: "T-1".into(),
                    }
                } else {
                    PermissionResource::MergeRequest {
                        workspace_id: "ws".into(),
                        merge_request_id: "MR-1".into(),
                    }
                }
            );
        }
    }

    #[tokio::test]
    async fn native_merge_request_rechecks_authority_after_successful_tool_read() {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![response(
            json!({"merge_request_id":"MR-1"}),
        )]));
        MergeRequestTool {
            client: client.clone(),
            kind: Kind::Show,
        }
        .execute(
            r#"{"merge_request_id":"MR-1"}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
        client
            .auth_denied
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let native = native_merge_request_tools(
            client.clone(),
            MergeRequestFeatureConfig {
                show: true,
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .find(|tool| tool.kind == Kind::Show)
        .unwrap();
        let result = execute_native_merge_request_tool(
            &native,
            &BTreeMap::new(),
            Some("MR-1"),
            WipCallContext {
                result_namespaces: Default::default(),
                execution: ToolExecutionContext::direct(),
                security_context: "test".into(),
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::PermissionDenied,
                ..
            }))
        ));
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        assert_eq!(client.authorizations.lock().unwrap().len(), 2);
    }
}

#[cfg(test)]
mod query_tests;
