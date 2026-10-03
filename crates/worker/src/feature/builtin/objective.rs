//!
//! Objective tool registration backed by Workspace API authority.
//!
//! Objectives are project-level planning context. Runtime Workers may not know
//! local `.yoi/objectives` paths, so model-visible Objective tools go through
//! the scoped Workspace API.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use manifest::{ToolPermissionAction, ToolPermissionConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use wip_protocol::{
    Documentation, INTERFACE_FORMAT_V1, InterfaceDescriptor, Object, OperationDeclaration,
    ParameterDeclaration, ProtocolError, ProtocolErrorCode, ReturnDeclaration, TypeExpr, Value,
};

use crate::permission::permission_action_for;
use crate::wip::{
    WipCallContext, WipDynamicItem, WipDynamicItemResolver, WipDynamicMount, WipFeatureRoute,
    WipMountError, WipMountRegistry, WipOperationError, WipOperationHandler, WipOperationOutput,
    WipProjection, WipProjectionKind, json_to_wip,
};
use crate::worker::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod};

use super::resource_projection::{project_objective_detail, project_objective_query};

#[derive(Clone, Debug)]
pub struct WorkspaceHttpObjectiveBackend {
    client: Arc<dyn WorkspaceClient>,
}

impl WorkspaceHttpObjectiveBackend {
    pub fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self { client }
    }

    async fn query_value(
        &self,
        input: &QueryObjectiveInput,
    ) -> Result<serde_json::Value, WorkspaceObjectiveBackendError> {
        let url = format!(
            "/api/w/{}/objectives/query",
            self.client.workspace_id().unwrap_or_default()
        );
        let response = send_json::<QueryObjectiveInput, serde_json::Value>(
            self.client.as_ref(),
            reqwest::Method::POST,
            &url,
            input,
        )
        .await?;
        let response = project_objective_query(response)
            .map_err(WorkspaceObjectiveBackendError::Projection)?;
        serde_json::to_value(response).map_err(Into::into)
    }

    async fn list(&self, input: QueryObjectiveInput) -> Result<ToolOutput, ToolError> {
        let response = self.query_value(&input).await.map_err(backend_error)?;
        Ok(ToolOutput {
            summary: "Queried Objectives".to_string(),
            content: Some(serde_json::to_string_pretty(&response).map_err(decode_error)?),
            attachments: Vec::new(),
        })
    }

    async fn read_value(
        &self,
        id: &str,
        event_limit: Option<usize>,
        event_cursor: Option<String>,
    ) -> Result<serde_json::Value, WorkspaceObjectiveBackendError> {
        let id = validate_backend_id(id)?;
        let url = format!("{}/show", self.objective_url(id));
        let response = send_json::<ObjectiveShowRequest, serde_json::Value>(
            self.client.as_ref(),
            reqwest::Method::POST,
            &url,
            &ObjectiveShowRequest {
                event_limit,
                event_cursor,
            },
        )
        .await?;
        let response = project_objective_detail(response)
            .map_err(WorkspaceObjectiveBackendError::Projection)?;
        serde_json::to_value(response).map_err(Into::into)
    }

    async fn show(&self, input: ShowObjectiveInput) -> Result<ToolOutput, ToolError> {
        let id = validate_id(&input.id, "ShowObjective")?;
        let response = self
            .read_value(id, input.event_limit, input.event_cursor)
            .await
            .map_err(backend_error)?;
        let objective_ref = response
            .get("objective")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(id);
        Ok(ToolOutput {
            summary: format!("Read objective {objective_ref}"),
            content: Some(serde_json::to_string_pretty(&response).map_err(decode_error)?),
            attachments: Vec::new(),
        })
    }

    async fn create_value(
        &self,
        input: &ObjectiveCreateInput,
    ) -> Result<serde_json::Value, WorkspaceObjectiveBackendError> {
        if input.title.trim().is_empty() {
            return Err(WorkspaceObjectiveBackendError::InvalidArgument(
                "ObjectiveCreate requires non-empty title".to_string(),
            ));
        }
        let url = format!(
            "/api/w/{}/objectives",
            self.client.workspace_id().unwrap_or_default()
        );
        let response = send_json::<ObjectiveCreateInput, serde_json::Value>(
            self.client.as_ref(),
            reqwest::Method::POST,
            &url,
            input,
        )
        .await?;
        project_objective_value(response)
    }

    async fn create(&self, input: ObjectiveCreateInput) -> Result<ToolOutput, ToolError> {
        let response = self.create_value(&input).await.map_err(backend_error)?;
        let objective_ref = objective_ref(&response).map_err(ToolError::ExecutionFailed)?;
        objective_output(format!("Created objective {objective_ref}"), &response)
    }

    async fn edit_value(
        &self,
        id: &str,
        input: &ObjectiveEditRequest,
    ) -> Result<serde_json::Value, WorkspaceObjectiveBackendError> {
        let id = validate_backend_id(id)?;
        if input.title.is_none() && input.old_string.is_none() && input.new_string.is_none() {
            return Err(WorkspaceObjectiveBackendError::InvalidArgument(
                "ObjectiveEdit requires title or old_string/new_string".to_string(),
            ));
        }
        let response = send_json::<ObjectiveEditRequest, serde_json::Value>(
            self.client.as_ref(),
            reqwest::Method::PATCH,
            &self.objective_url(id),
            input,
        )
        .await?;
        project_objective_value(response)
    }

    async fn edit(&self, input: ObjectiveEditInput) -> Result<ToolOutput, ToolError> {
        let id = validate_id(&input.id, "ObjectiveEdit")?;
        let body = ObjectiveEditRequest {
            title: input.title,
            old_string: input.old_string,
            new_string: input.new_string,
            replace_all: input.replace_all,
        };
        let response = self.edit_value(id, &body).await.map_err(backend_error)?;
        let objective_ref = objective_ref(&response).map_err(ToolError::ExecutionFailed)?;
        objective_output(format!("Edited objective {objective_ref}"), &response)
    }

    async fn set_state_value(
        &self,
        id: &str,
        state: &str,
    ) -> Result<serde_json::Value, WorkspaceObjectiveBackendError> {
        let id = validate_backend_id(id)?;
        if state.trim().is_empty() {
            return Err(WorkspaceObjectiveBackendError::InvalidArgument(
                "ObjectiveSetState requires non-empty state".to_string(),
            ));
        }
        let response = send_json::<ObjectiveSetStateRequest, serde_json::Value>(
            self.client.as_ref(),
            reqwest::Method::POST,
            &format!("{}/state", self.objective_url(id)),
            &ObjectiveSetStateRequest {
                state: state.to_string(),
            },
        )
        .await?;
        project_objective_value(response)
    }

    async fn set_state(&self, input: ObjectiveSetStateInput) -> Result<ToolOutput, ToolError> {
        let id = validate_id(&input.id, "ObjectiveSetState")?;
        let response = self
            .set_state_value(id, &input.state)
            .await
            .map_err(backend_error)?;
        let objective_ref = objective_ref(&response).map_err(ToolError::ExecutionFailed)?;
        objective_output(
            format!("Updated objective {objective_ref} state"),
            &response,
        )
    }

    async fn link_ticket_value(
        &self,
        id: &str,
        ticket_id: &str,
    ) -> Result<(String, serde_json::Value), WorkspaceObjectiveBackendError> {
        let id = validate_backend_id(id)?;
        let ticket_id = validate_backend_id(ticket_id)?;
        let ticket_resource_key = self.ticket_resource_key_raw(ticket_id).await?;
        let response = send_json::<ObjectiveLinkTicketRequest, serde_json::Value>(
            self.client.as_ref(),
            reqwest::Method::POST,
            &format!("{}/ticket-links", self.objective_url(id)),
            &ObjectiveLinkTicketRequest {
                ticket_id: ticket_id.to_string(),
            },
        )
        .await?;
        Ok((ticket_resource_key, project_objective_value(response)?))
    }

    async fn link_ticket(&self, input: ObjectiveLinkTicketInput) -> Result<ToolOutput, ToolError> {
        let id = validate_id(&input.id, "ObjectiveLinkTicket")?;
        let ticket_id = validate_id(&input.ticket_id, "ObjectiveLinkTicket")?;
        let (ticket_resource_key, response) = self
            .link_ticket_value(id, ticket_id)
            .await
            .map_err(backend_error)?;
        let objective_ref = objective_ref(&response).map_err(ToolError::ExecutionFailed)?;
        objective_output(
            format!("Linked ticket {ticket_resource_key} to objective {objective_ref}"),
            &response,
        )
    }

    async fn unlink_ticket_value(
        &self,
        id: &str,
        ticket_id: &str,
    ) -> Result<(String, serde_json::Value), WorkspaceObjectiveBackendError> {
        let id = validate_backend_id(id)?;
        let ticket_id = validate_backend_id(ticket_id)?;
        let ticket_resource_key = self.ticket_resource_key_raw(ticket_id).await?;
        let response = delete_json::<serde_json::Value>(
            self.client.as_ref(),
            &format!(
                "{}/ticket-links/{}",
                self.objective_url(id),
                encode_path_segment(ticket_id)
            ),
        )
        .await?;
        Ok((ticket_resource_key, project_objective_value(response)?))
    }

    async fn unlink_ticket(
        &self,
        input: ObjectiveUnlinkTicketInput,
    ) -> Result<ToolOutput, ToolError> {
        let id = validate_id(&input.id, "ObjectiveUnlinkTicket")?;
        let ticket_id = validate_id(&input.ticket_id, "ObjectiveUnlinkTicket")?;
        let (ticket_resource_key, response) = self
            .unlink_ticket_value(id, ticket_id)
            .await
            .map_err(backend_error)?;
        let objective_ref = objective_ref(&response).map_err(ToolError::ExecutionFailed)?;
        objective_output(
            format!("Unlinked ticket {ticket_resource_key} from objective {objective_ref}"),
            &response,
        )
    }

    async fn ticket_resource_key_raw(
        &self,
        ticket_reference: &str,
    ) -> Result<String, WorkspaceObjectiveBackendError> {
        let workspace_id = self.client.workspace_id().unwrap_or_default();
        let response: serde_json::Value =
            decode_response(self.client.execute(WorkspaceRequest::get(format!(
                "/api/w/{workspace_id}/tickets/{}",
                encode_path_segment(ticket_reference)
            )))?)?;
        response
            .get("resource_key")
            .or_else(|| {
                response
                    .get("meta")
                    .and_then(|meta| meta.get("resource_key"))
            })
            .and_then(serde_json::Value::as_str)
            .filter(|key| is_canonical_resource_key(key, "T-"))
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                WorkspaceObjectiveBackendError::Projection(
                    "required T- key is unavailable".to_string(),
                )
            })
    }

    fn objective_url(&self, id: &str) -> String {
        let workspace_id = self.client.workspace_id().unwrap_or_default();
        format!(
            "/api/w/{workspace_id}/objectives/{}",
            encode_path_segment(id)
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceObjectiveBackendError {
    #[error("invalid Objective operation: {0}")]
    InvalidArgument(String),
    #[error("workspace objective backend request failed: {0}")]
    Request(#[from] crate::worker::WorkspaceClientError),
    #[error("workspace objective backend returned HTTP {status}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("decode objective backend response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("project objective backend response: {0}")]
    Projection(String),
}

fn decode_error(error: serde_json::Error) -> ToolError {
    ToolError::ExecutionFailed(format!("decode objective backend response: {error}"))
}

fn backend_error(error: WorkspaceObjectiveBackendError) -> ToolError {
    match error {
        WorkspaceObjectiveBackendError::InvalidArgument(message) => {
            ToolError::InvalidArgument(message)
        }
        error => ToolError::ExecutionFailed(error.to_string()),
    }
}

async fn send_json<B: Serialize, T: for<'de> Deserialize<'de>>(
    client: &dyn WorkspaceClient,
    method: reqwest::Method,
    path: &str,
    body: &B,
) -> Result<T, WorkspaceObjectiveBackendError> {
    let method = match method {
        reqwest::Method::POST => WorkspaceRequestMethod::Post,
        reqwest::Method::PUT => WorkspaceRequestMethod::Put,
        reqwest::Method::PATCH => WorkspaceRequestMethod::Patch,
        reqwest::Method::DELETE => WorkspaceRequestMethod::Delete,
        _ => WorkspaceRequestMethod::Get,
    };
    decode_response(client.execute(WorkspaceRequest::json(
        method,
        path,
        serde_json::to_string(body)?,
    ))?)
}

async fn delete_json<T: for<'de> Deserialize<'de>>(
    client: &dyn WorkspaceClient,
    path: &str,
) -> Result<T, WorkspaceObjectiveBackendError> {
    decode_response(client.execute(WorkspaceRequest {
        method: WorkspaceRequestMethod::Delete,
        path: path.to_string(),
        body: None,
    })?)
}

fn decode_response<T: for<'de> Deserialize<'de>>(
    response: crate::worker::WorkspaceResponse,
) -> Result<T, WorkspaceObjectiveBackendError> {
    let status = reqwest::StatusCode::from_u16(response.status)
        .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    if !response.is_success() {
        return Err(WorkspaceObjectiveBackendError::Http {
            status,
            body: response.body,
        });
    }
    serde_json::from_str(&response.body).map_err(Into::into)
}

fn is_canonical_resource_key(resource_key: &str, prefix: &str) -> bool {
    resource_key.strip_prefix(prefix).is_some_and(|sequence| {
        !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn project_objective_value(
    response: serde_json::Value,
) -> Result<serde_json::Value, WorkspaceObjectiveBackendError> {
    let projected =
        project_objective_detail(response).map_err(WorkspaceObjectiveBackendError::Projection)?;
    serde_json::to_value(projected).map_err(Into::into)
}

fn objective_ref(response: &serde_json::Value) -> Result<&str, String> {
    response
        .get("objective")
        .and_then(serde_json::Value::as_str)
        .filter(|key| is_canonical_resource_key(key, "O-"))
        .ok_or_else(|| "required O- key is unavailable".to_string())
}

fn objective_output(
    summary: String,
    response: &serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    let objective = objective_ref(response).map_err(ToolError::ExecutionFailed)?;
    let title = response
        .get("title")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ToolError::ExecutionFailed("Objective title is unavailable".into()))?;
    let state = response
        .get("state")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ToolError::ExecutionFailed("Objective state is unavailable".into()))?;
    let projected = serde_json::json!({
        "objective": objective,
        "title": title,
        "state": state,
    });
    Ok(ToolOutput {
        summary,
        content: Some(serde_json::to_string_pretty(&projected).map_err(decode_error)?),
        attachments: Vec::new(),
    })
}

fn validate_backend_id(id: &str) -> Result<&str, WorkspaceObjectiveBackendError> {
    let id = id.trim();
    if id.is_empty() || id.contains('/') {
        return Err(WorkspaceObjectiveBackendError::InvalidArgument(
            "Objective reference must be non-empty and contain no '/'".into(),
        ));
    }
    Ok(id)
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(&mut encoded, "%{byte:02X}").expect("write to String");
        }
    }
    encoded
}

fn validate_id<'a>(id: &'a str, tool_name: &str) -> Result<&'a str, ToolError> {
    let id = id.trim();
    if id.is_empty() || id.contains('/') {
        return Err(ToolError::InvalidArgument(format!(
            "{tool_name} requires a non-empty Objective reference without '/'"
        )));
    }
    Ok(id)
}

pub fn workspace_http_objective_tools(client: Arc<dyn WorkspaceClient>) -> Vec<ToolDefinition> {
    let backend = WorkspaceHttpObjectiveBackend::new(client);
    vec![
        objective_tool(
            "QueryObjective",
            LIST_DESCRIPTION,
            list_schema(),
            backend.clone(),
            ObjectiveOperation::List,
        ),
        objective_tool(
            "ShowObjective",
            SHOW_DESCRIPTION,
            show_schema(),
            backend.clone(),
            ObjectiveOperation::Show,
        ),
        objective_tool(
            "ObjectiveCreate",
            CREATE_DESCRIPTION,
            create_schema(),
            backend.clone(),
            ObjectiveOperation::Create,
        ),
        objective_tool(
            "ObjectiveEdit",
            EDIT_DESCRIPTION,
            edit_schema(),
            backend.clone(),
            ObjectiveOperation::Edit,
        ),
        objective_tool(
            "ObjectiveSetState",
            SET_STATE_DESCRIPTION,
            set_state_schema(),
            backend.clone(),
            ObjectiveOperation::SetState,
        ),
        objective_tool(
            "ObjectiveLinkTicket",
            LINK_TICKET_DESCRIPTION,
            link_ticket_schema(),
            backend.clone(),
            ObjectiveOperation::LinkTicket,
        ),
        objective_tool(
            "ObjectiveUnlinkTicket",
            UNLINK_TICKET_DESCRIPTION,
            unlink_ticket_schema(),
            backend,
            ObjectiveOperation::UnlinkTicket,
        ),
    ]
}

const OBJECTIVE_COLLECTION_INTERFACE: &str = "yoi.objective/collection/v1";
const OBJECTIVE_ITEM_INTERFACE: &str = "yoi.objective/item/v1";
const OBJECTIVE_TOOL_NAMES: [&str; 7] = [
    "QueryObjective",
    "ShowObjective",
    "ObjectiveCreate",
    "ObjectiveEdit",
    "ObjectiveSetState",
    "ObjectiveLinkTicket",
    "ObjectiveUnlinkTicket",
];

type ObjectiveRevisions = Arc<Mutex<HashMap<String, String>>>;

/// Contribute the Objective Feature's native collection and item projections to
/// the Host-owned WIP registry. Ordinary Objective tools remain registered for
/// Tool mode and are claimed only from the WIP compatibility projection.
pub fn mount_workspace_http_objective_wip(
    registry: &mut WipMountRegistry,
    client: Arc<dyn WorkspaceClient>,
    permissions: Option<ToolPermissionConfig>,
    feature_route: &WipFeatureRoute,
) -> Result<(), WipMountError> {
    let collection_route = feature_route.child("objectives")?;
    let backend = WorkspaceHttpObjectiveBackend::new(client);
    let revisions = Arc::new(Mutex::new(HashMap::new()));
    let collection_descriptor = objective_collection_descriptor();
    let collection_validator = descriptor_validator(&collection_descriptor);
    let collection_handler = Arc::new(ObjectiveCollectionWipHandler {
        backend: backend.clone(),
        permissions: permissions.clone(),
        collection_route: collection_route.clone(),
        revisions: Arc::clone(&revisions),
    });
    registry.mount(WipProjection {
        route: collection_route.clone(),
        capability: "objective:collection".into(),
        kind: WipProjectionKind::Native,
        object: Object {
            name: collection_route
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string(),
            description: Some(
                "Authoritative Objective collection; search and create through Backend authority"
                    .into(),
            ),
            interfaces: vec![OBJECTIVE_COLLECTION_INTERFACE.into()],
            r#ref: Some("objective:collection".into()),
            validator: Some(route_validator(&collection_route, "collection")),
        },
        interface: OBJECTIVE_COLLECTION_INTERFACE.into(),
        descriptor: collection_descriptor,
        interface_validator: Some(collection_validator),
        handler: collection_handler,
    })?;

    let item_descriptor = objective_item_descriptor();
    let item_validator = descriptor_validator(&item_descriptor);
    registry.mount_dynamic(WipDynamicMount {
        collection_route: collection_route.clone(),
        capability: "objective:item".into(),
        interface: OBJECTIVE_ITEM_INTERFACE.into(),
        descriptor: item_descriptor,
        interface_validator: Some(item_validator),
        resolver: Arc::new(ObjectiveItemResolver {
            backend,
            permissions,
            collection_route: collection_route.clone(),
            revisions,
        }),
    })?;
    registry.replace_compatibility_tools(&collection_route, OBJECTIVE_TOOL_NAMES)?;
    Ok(())
}

struct ObjectiveCollectionWipHandler {
    backend: WorkspaceHttpObjectiveBackend,
    permissions: Option<ToolPermissionConfig>,
    collection_route: String,
    revisions: ObjectiveRevisions,
}

#[async_trait]
impl WipOperationHandler for ObjectiveCollectionWipHandler {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        _context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let value = match operation {
            "query" => {
                let limit = optional_usize_argument(arguments, "limit")?;
                ensure_optional_range("limit", limit, 1, 100)?;
                let sort = optional_string_argument(arguments, "sort")?;
                if sort.as_deref().is_some_and(|sort| {
                    !matches!(
                        sort,
                        "relevance" | "updated_desc" | "created_desc" | "title"
                    )
                }) {
                    return Err(protocol_failure(
                        ProtocolErrorCode::InvalidArguments,
                        "`sort` is not an Objective query ordering",
                    ));
                }
                let input = QueryObjectiveInput {
                    query: optional_string_argument(arguments, "query")?,
                    states: string_list_argument(arguments, "states")?.unwrap_or_default(),
                    linked_ticket_id: optional_string_argument(arguments, "linked_ticket_id")?,
                    updated_after: optional_string_argument(arguments, "updated_after")?,
                    updated_before: optional_string_argument(arguments, "updated_before")?,
                    sort,
                    limit,
                    cursor: optional_string_argument(arguments, "cursor")?,
                };
                let permission_input = serde_json::to_value(&input)
                    .map_err(|error| WipOperationError::OutcomeUnknown(error.to_string()))?;
                authorize_native(&self.permissions, "QueryObjective", &permission_input)?;
                let mut response = self
                    .backend
                    .query_value(&input)
                    .await
                    .map_err(map_backend_error)?;
                if let Some(objectives) =
                    response.get_mut("objectives").and_then(Json::as_array_mut)
                {
                    for objective in objectives {
                        if let Some(reference) = objective
                            .get("objective")
                            .and_then(Json::as_str)
                            .map(ToOwned::to_owned)
                        {
                            objective
                                .as_object_mut()
                                .expect("projected Objective is an object")
                                .insert(
                                    "path".into(),
                                    Json::String(format!(
                                        "{}/{}",
                                        self.collection_route, reference
                                    )),
                                );
                        }
                    }
                }
                response
            }
            "create" => {
                let input = ObjectiveCreateInput {
                    title: required_string_argument(arguments, "title")?,
                    body_md: optional_string_argument(arguments, "body_md")?.unwrap_or_default(),
                    state: optional_string_argument(arguments, "state")?
                        .unwrap_or_else(default_state),
                    linked_tickets: string_list_argument(arguments, "linked_tickets")?
                        .unwrap_or_default(),
                };
                let permission_input = serde_json::to_value(&input)
                    .map_err(|error| WipOperationError::OutcomeUnknown(error.to_string()))?;
                authorize_native(&self.permissions, "ObjectiveCreate", &permission_input)?;
                let mut response = self
                    .backend
                    .create_value(&input)
                    .await
                    .map_err(map_backend_error)?;
                record_revision(&self.revisions, None, &response);
                if let Some(reference) = response
                    .get("objective")
                    .and_then(Json::as_str)
                    .map(ToOwned::to_owned)
                {
                    response
                        .as_object_mut()
                        .expect("projected Objective is an object")
                        .insert(
                            "path".into(),
                            Json::String(format!("{}/{}", self.collection_route, reference)),
                        );
                }
                response
            }
            _ => return Err(operation_not_found()),
        };
        Ok(WipOperationOutput::native(
            json_to_wip(&value).map_err(WipOperationError::OutcomeUnknown)?,
        ))
    }
}

struct ObjectiveItemResolver {
    backend: WorkspaceHttpObjectiveBackend,
    permissions: Option<ToolPermissionConfig>,
    collection_route: String,
    revisions: ObjectiveRevisions,
}

impl WipDynamicItemResolver for ObjectiveItemResolver {
    fn resolve(&self, item_reference: &str) -> Option<WipDynamicItem> {
        if !is_objective_route_reference(item_reference) {
            return None;
        }
        let revision = self
            .revisions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(item_reference)
            .cloned()
            .unwrap_or_else(|| "unobserved".into());
        let route = format!("{}/{}", self.collection_route, item_reference);
        Some(WipDynamicItem {
            object: Object {
                name: item_reference.to_string(),
                description: Some("Authoritative Objective bound to this object route".into()),
                interfaces: vec![OBJECTIVE_ITEM_INTERFACE.into()],
                r#ref: Some(format!("objective:{item_reference}")),
                validator: Some(route_validator(&route, &revision)),
            },
            handler: Arc::new(ObjectiveItemWipHandler {
                backend: self.backend.clone(),
                permissions: self.permissions.clone(),
                objective_reference: item_reference.to_string(),
                revisions: Arc::clone(&self.revisions),
            }),
        })
    }
}

struct ObjectiveItemWipHandler {
    backend: WorkspaceHttpObjectiveBackend,
    permissions: Option<ToolPermissionConfig>,
    objective_reference: String,
    revisions: ObjectiveRevisions,
}

#[async_trait]
impl WipOperationHandler for ObjectiveItemWipHandler {
    async fn call(
        &self,
        operation: &str,
        arguments: &BTreeMap<String, Value>,
        _context: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        let (tool_name, permission_input, response) = match operation {
            "read" => {
                let event_limit = optional_usize_argument(arguments, "event_limit")?;
                ensure_optional_range("event_limit", event_limit, 1, 50)?;
                let event_cursor = optional_string_argument(arguments, "event_cursor")?;
                let permission_input = json!({
                    "id": self.objective_reference,
                    "event_limit": event_limit,
                    "event_cursor": event_cursor,
                });
                authorize_native(&self.permissions, "ShowObjective", &permission_input)?;
                let response = self
                    .backend
                    .read_value(&self.objective_reference, event_limit, event_cursor)
                    .await
                    .map_err(map_backend_error)?;
                ("ShowObjective", permission_input, response)
            }
            "edit" => {
                let input = ObjectiveEditRequest {
                    title: optional_string_argument(arguments, "title")?,
                    old_string: optional_string_argument(arguments, "old_string")?,
                    new_string: optional_string_argument(arguments, "new_string")?,
                    replace_all: optional_bool_argument(arguments, "replace_all")?.unwrap_or(false),
                };
                let permission_input = json!({
                    "id": self.objective_reference,
                    "title": input.title,
                    "old_string": input.old_string,
                    "new_string": input.new_string,
                    "replace_all": input.replace_all,
                });
                authorize_native(&self.permissions, "ObjectiveEdit", &permission_input)?;
                let response = self
                    .backend
                    .edit_value(&self.objective_reference, &input)
                    .await
                    .map_err(map_backend_error)?;
                ("ObjectiveEdit", permission_input, response)
            }
            "set_state" => {
                let state = required_string_argument(arguments, "state")?;
                let permission_input = json!({"id": self.objective_reference, "state": state});
                authorize_native(&self.permissions, "ObjectiveSetState", &permission_input)?;
                let response = self
                    .backend
                    .set_state_value(&self.objective_reference, &state)
                    .await
                    .map_err(map_backend_error)?;
                ("ObjectiveSetState", permission_input, response)
            }
            "link_ticket" | "unlink_ticket" => {
                let ticket_id = required_string_argument(arguments, "ticket_id")?;
                let permission_input = json!({
                    "id": self.objective_reference,
                    "ticket_id": ticket_id,
                });
                let (tool_name, response) = if operation == "link_ticket" {
                    authorize_native(&self.permissions, "ObjectiveLinkTicket", &permission_input)?;
                    let (_, response) = self
                        .backend
                        .link_ticket_value(&self.objective_reference, &ticket_id)
                        .await
                        .map_err(map_backend_error)?;
                    ("ObjectiveLinkTicket", response)
                } else {
                    authorize_native(
                        &self.permissions,
                        "ObjectiveUnlinkTicket",
                        &permission_input,
                    )?;
                    let (_, response) = self
                        .backend
                        .unlink_ticket_value(&self.objective_reference, &ticket_id)
                        .await
                        .map_err(map_backend_error)?;
                    ("ObjectiveUnlinkTicket", response)
                };
                (tool_name, permission_input, response)
            }
            _ => return Err(operation_not_found()),
        };
        let _ = (tool_name, permission_input);
        record_revision(&self.revisions, Some(&self.objective_reference), &response);
        Ok(WipOperationOutput::native(
            json_to_wip(&response).map_err(WipOperationError::OutcomeUnknown)?,
        ))
    }
}

fn authorize_native(
    permissions: &Option<ToolPermissionConfig>,
    tool_name: &str,
    arguments: &Json,
) -> Result<(), WipOperationError> {
    let Some(permissions) = permissions else {
        return Ok(());
    };
    match permission_action_for(permissions, tool_name, arguments) {
        ToolPermissionAction::Allow => Ok(()),
        ToolPermissionAction::Deny => Err(protocol_failure(
            ProtocolErrorCode::PermissionDenied,
            format!("permission denied for projected tool `{tool_name}`"),
        )),
        ToolPermissionAction::Ask => Err(protocol_failure(
            ProtocolErrorCode::PermissionDenied,
            format!(
                "permission approval is unavailable for projected tool `{tool_name}`; denied fail-closed"
            ),
        )),
    }
}

fn map_backend_error(error: WorkspaceObjectiveBackendError) -> WipOperationError {
    match error {
        WorkspaceObjectiveBackendError::InvalidArgument(message) => {
            protocol_failure(ProtocolErrorCode::InvalidArguments, message)
        }
        WorkspaceObjectiveBackendError::Http { status, body }
            if status == reqwest::StatusCode::NOT_FOUND =>
        {
            protocol_failure(ProtocolErrorCode::NotFound, body)
        }
        WorkspaceObjectiveBackendError::Http { status, body }
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN =>
        {
            protocol_failure(ProtocolErrorCode::PermissionDenied, body)
        }
        WorkspaceObjectiveBackendError::Http { status, body } if status.is_client_error() => {
            protocol_failure(
                ProtocolErrorCode::InvalidArguments,
                format!("workspace objective backend returned HTTP {status}: {body}"),
            )
        }
        error => WipOperationError::OutcomeUnknown(error.to_string()),
    }
}

fn protocol_failure(code: ProtocolErrorCode, message: impl Into<String>) -> WipOperationError {
    WipOperationError::Protocol(ProtocolError {
        code,
        message: message.into(),
    })
}

fn operation_not_found() -> WipOperationError {
    protocol_failure(
        ProtocolErrorCode::OperationNotFound,
        "operation is not published by the Objective interface",
    )
}

fn required_string_argument(
    arguments: &BTreeMap<String, Value>,
    name: &str,
) -> Result<String, WipOperationError> {
    optional_string_argument(arguments, name)?.ok_or_else(|| {
        protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("missing required `{name}` argument"),
        )
    })
}

fn optional_string_argument(
    arguments: &BTreeMap<String, Value>,
    name: &str,
) -> Result<Option<String>, WipOperationError> {
    match arguments.get(name) {
        None | Some(Value::Unit) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("`{name}` must be a string"),
        )),
    }
}

fn optional_bool_argument(
    arguments: &BTreeMap<String, Value>,
    name: &str,
) -> Result<Option<bool>, WipOperationError> {
    match arguments.get(name) {
        None | Some(Value::Unit) => Ok(None),
        Some(Value::Boolean(value)) => Ok(Some(*value)),
        Some(_) => Err(protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("`{name}` must be a boolean"),
        )),
    }
}

fn optional_usize_argument(
    arguments: &BTreeMap<String, Value>,
    name: &str,
) -> Result<Option<usize>, WipOperationError> {
    match arguments.get(name) {
        None | Some(Value::Unit) => Ok(None),
        Some(Value::Integer(value)) => usize::try_from(*value).map(Some).map_err(|_| {
            protocol_failure(
                ProtocolErrorCode::InvalidArguments,
                format!("`{name}` must be a non-negative integer"),
            )
        }),
        Some(_) => Err(protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("`{name}` must be an integer"),
        )),
    }
}

fn ensure_optional_range(
    name: &str,
    value: Option<usize>,
    minimum: usize,
    maximum: usize,
) -> Result<(), WipOperationError> {
    if value.is_some_and(|value| !(minimum..=maximum).contains(&value)) {
        return Err(protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("`{name}` must be between {minimum} and {maximum}"),
        ));
    }
    Ok(())
}

fn string_list_argument(
    arguments: &BTreeMap<String, Value>,
    name: &str,
) -> Result<Option<Vec<String>>, WipOperationError> {
    match arguments.get(name) {
        None | Some(Value::Unit) => Ok(None),
        Some(Value::List(values)) => values
            .iter()
            .map(|value| match value {
                Value::String(value) => Ok(value.clone()),
                _ => Err(protocol_failure(
                    ProtocolErrorCode::InvalidArguments,
                    format!("`{name}` entries must be strings"),
                )),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("`{name}` must be a list"),
        )),
    }
}

fn record_revision(revisions: &ObjectiveRevisions, bound_reference: Option<&str>, response: &Json) {
    let Some(revision) = response.get("revision").and_then(Json::as_str) else {
        return;
    };
    let mut revisions = revisions.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(reference) = bound_reference {
        revisions.insert(reference.to_string(), revision.to_string());
    }
    if let Some(reference) = response.get("objective").and_then(Json::as_str) {
        revisions.insert(reference.to_string(), revision.to_string());
    }
}

fn is_objective_route_reference(reference: &str) -> bool {
    is_canonical_resource_key(reference, "O-")
        || (reference.len() >= 8
            && reference.len() <= 128
            && reference.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

fn route_validator(route: &str, revision: &str) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(route.as_bytes());
    digest.update([0]);
    digest.update(revision.as_bytes());
    digest.finalize().to_vec()
}

fn descriptor_validator(descriptor: &InterfaceDescriptor) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(format!("{descriptor:?}").as_bytes());
    digest.finalize().to_vec()
}

fn objective_collection_descriptor() -> InterfaceDescriptor {
    InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: Some(Documentation {
            summary: "Search and create authoritative Workspace Objectives".into(),
            details: Some(
                "Query results are bounded and include canonical item paths. Ticket references are data only and confer no Ticket authority."
                    .into(),
            ),
        }),
        types: Vec::new(),
        operations: vec![
            OperationDeclaration {
                name: "query".into(),
                documentation: Some(Documentation {
                    summary: LIST_DESCRIPTION.into(),
                    details: None,
                }),
                parameters: vec![
                    parameter("query", false, TypeExpr::String),
                    parameter("states", false, list_of(TypeExpr::String)),
                    parameter("linked_ticket_id", false, TypeExpr::String),
                    parameter("updated_after", false, TypeExpr::String),
                    parameter("updated_before", false, TypeExpr::String),
                    parameter("sort", false, TypeExpr::String),
                    parameter("limit", false, TypeExpr::Integer),
                    parameter("cursor", false, TypeExpr::String),
                ],
                returns: json_return("Bounded Objective query with cursor metadata and item paths"),
            },
            OperationDeclaration {
                name: "create".into(),
                documentation: Some(Documentation {
                    summary: CREATE_DESCRIPTION.into(),
                    details: None,
                }),
                parameters: vec![
                    parameter("title", true, TypeExpr::String),
                    parameter("body_md", false, TypeExpr::String),
                    parameter("state", false, TypeExpr::String),
                    parameter("linked_tickets", false, list_of(TypeExpr::String)),
                ],
                returns: json_return("Created Objective with revision and bounded context"),
            },
        ],
    }
}

fn objective_item_descriptor() -> InterfaceDescriptor {
    InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: Some(Documentation {
            summary: "Read and mutate the Objective bound to this object route".into(),
            details: Some(
                "The Objective identity is taken exclusively from the target path; operations do not accept a second Objective id."
                    .into(),
            ),
        }),
        types: Vec::new(),
        operations: vec![
            operation(
                "read",
                SHOW_DESCRIPTION,
                vec![
                    parameter("event_limit", false, TypeExpr::Integer),
                    parameter("event_cursor", false, TypeExpr::String),
                ],
            ),
            operation(
                "edit",
                EDIT_DESCRIPTION,
                vec![
                    parameter("title", false, TypeExpr::String),
                    parameter("old_string", false, TypeExpr::String),
                    parameter("new_string", false, TypeExpr::String),
                    parameter("replace_all", false, TypeExpr::Boolean),
                ],
            ),
            operation(
                "set_state",
                SET_STATE_DESCRIPTION,
                vec![parameter("state", true, TypeExpr::String)],
            ),
            operation(
                "link_ticket",
                LINK_TICKET_DESCRIPTION,
                vec![parameter("ticket_id", true, TypeExpr::String)],
            ),
            operation(
                "unlink_ticket",
                UNLINK_TICKET_DESCRIPTION,
                vec![parameter("ticket_id", true, TypeExpr::String)],
            ),
        ],
    }
}

fn operation(
    name: &str,
    summary: &str,
    parameters: Vec<ParameterDeclaration>,
) -> OperationDeclaration {
    OperationDeclaration {
        name: name.into(),
        documentation: Some(Documentation {
            summary: summary.into(),
            details: None,
        }),
        parameters,
        returns: json_return("Authoritative Objective detail with revision and bounded context"),
    }
}

fn parameter(name: &str, required: bool, r#type: TypeExpr) -> ParameterDeclaration {
    ParameterDeclaration {
        name: name.into(),
        required,
        documentation: None,
        r#type,
    }
}

fn list_of(items: TypeExpr) -> TypeExpr {
    TypeExpr::List {
        items: Box::new(items),
    }
}

fn json_return(summary: &str) -> ReturnDeclaration {
    ReturnDeclaration {
        documentation: Some(Documentation {
            summary: summary.into(),
            details: None,
        }),
        r#type: TypeExpr::Json,
    }
}

#[derive(Clone, Copy)]
enum ObjectiveOperation {
    List,
    Show,
    Create,
    Edit,
    SetState,
    LinkTicket,
    UnlinkTicket,
}

fn objective_tool(
    name: &'static str,
    description: &'static str,
    schema: serde_json::Value,
    backend: WorkspaceHttpObjectiveBackend,
    operation: ObjectiveOperation,
) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(name)
                .description(description)
                .input_schema(schema.clone()),
            Arc::new(WorkspaceHttpObjectiveTool {
                backend: backend.clone(),
                operation,
            }) as Arc<dyn Tool>,
        )
    })
}

#[derive(Clone)]
struct WorkspaceHttpObjectiveTool {
    backend: WorkspaceHttpObjectiveBackend,
    operation: ObjectiveOperation,
}

#[async_trait]
impl Tool for WorkspaceHttpObjectiveTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        match self.operation {
            ObjectiveOperation::List => {
                let input = parse_input::<QueryObjectiveInput>(input_json)?;
                self.backend.list(input).await
            }
            ObjectiveOperation::Show => {
                let input = parse_input::<ShowObjectiveInput>(input_json)?;
                self.backend.show(input).await
            }
            ObjectiveOperation::Create => {
                let input = parse_input::<ObjectiveCreateInput>(input_json)?;
                self.backend.create(input).await
            }
            ObjectiveOperation::Edit => {
                let input = parse_input::<ObjectiveEditInput>(input_json)?;
                self.backend.edit(input).await
            }
            ObjectiveOperation::SetState => {
                let input = parse_input::<ObjectiveSetStateInput>(input_json)?;
                self.backend.set_state(input).await
            }
            ObjectiveOperation::LinkTicket => {
                let input = parse_input::<ObjectiveLinkTicketInput>(input_json)?;
                self.backend.link_ticket(input).await
            }
            ObjectiveOperation::UnlinkTicket => {
                let input = parse_input::<ObjectiveUnlinkTicketInput>(input_json)?;
                self.backend.unlink_ticket(input).await
            }
        }
    }
}

fn parse_input<T: for<'de> Deserialize<'de>>(input: &str) -> Result<T, ToolError> {
    serde_json::from_str(input).map_err(|error| ToolError::InvalidArgument(error.to_string()))
}

const LIST_DESCRIPTION: &str = "Query authoritative Objectives with bounded typed filters, stable snippets, linked-Ticket context, and cursor metadata.";
const SHOW_DESCRIPTION: &str = "Show one authoritative Objective with its revision, full linked-Ticket context, bounded body, and paged event metadata.";
const CREATE_DESCRIPTION: &str =
    "Create an Objective record through Backend Workspace API authority.";
const EDIT_DESCRIPTION: &str =
    "Partially edit an Objective title and/or body through Backend Workspace API authority.";
const SET_STATE_DESCRIPTION: &str =
    "Set an Objective state through Backend Workspace API authority.";
const LINK_TICKET_DESCRIPTION: &str =
    "Link a Ticket reference to an Objective through Backend Workspace API authority.";
const UNLINK_TICKET_DESCRIPTION: &str =
    "Unlink a Ticket reference from an Objective through Backend Workspace API authority.";

fn list_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "properties":{
            "query":{"type":["string","null"]},
            "states":{"type":"array","items":{"type":"string"},"default":[]},
            "linked_ticket_id":{"type":["string","null"],"description":"Linked Ticket reference. Prefer T-*; canonical internal ids remain accepted for compatibility."},
            "updated_after":{"type":["string","null"]},
            "updated_before":{"type":["string","null"]},
            "sort":{"type":["string","null"],"enum":["relevance","updated_desc","created_desc","title",null]},
            "limit":{"type":["integer","null"],"minimum":1,"maximum":100},
            "cursor":{"type":["string","null"]}
        }
    })
}

fn show_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required":["id"],
        "properties":{
            "id":{"type":"string","description":"Objective reference. Prefer O-*; canonical internal ids remain accepted for compatibility."},
            "event_limit":{"type":["integer","null"],"minimum":1,"maximum":50},
            "event_cursor":{"type":["string","null"]}
        }
    })
}

fn create_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required":["title"],
        "properties":{
            "title":{"type":"string","minLength":1},
            "body_md":{"type":"string"},
            "state":{"type":"string","default":"active"},
            "linked_tickets":{"type":"array","items":{"type":"string"},"description":"Linked Ticket references. Prefer T-*; canonical internal ids remain accepted for compatibility."}
        }
    })
}

fn edit_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required":["id"],
        "properties":{
            "id":{"type":"string","description":"Objective reference. Prefer O-*; canonical internal ids remain accepted for compatibility."},
            "title":{"type":["string","null"]},
            "old_string":{"type":["string","null"]},
            "new_string":{"type":["string","null"]},
            "replace_all":{"type":"boolean","default":false}
        }
    })
}

fn set_state_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required":["id","state"],
        "properties":{
            "id":{"type":"string","description":"Objective reference. Prefer O-*; canonical internal ids remain accepted for compatibility."},
            "state":{"type":"string","minLength":1}
        }
    })
}

fn link_ticket_schema() -> serde_json::Value {
    id_ticket_schema(&["id", "ticket_id"])
}

fn unlink_ticket_schema() -> serde_json::Value {
    id_ticket_schema(&["id", "ticket_id"])
}

fn id_ticket_schema(required: &[&str]) -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required": required,
        "properties":{
            "id":{"type":"string","description":"Objective reference. Prefer O-*; canonical internal ids remain accepted for compatibility."},
            "ticket_id":{"type":"string","description":"Ticket reference. Prefer T-*; canonical internal ids remain accepted for compatibility."}
        }
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct QueryObjectiveInput {
    query: Option<String>,
    #[serde(default)]
    states: Vec<String>,
    linked_ticket_id: Option<String>,
    updated_after: Option<String>,
    updated_before: Option<String>,
    sort: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ShowObjectiveInput {
    id: String,
    event_limit: Option<usize>,
    event_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct ObjectiveShowRequest {
    event_limit: Option<usize>,
    event_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ObjectiveCreateInput {
    title: String,
    #[serde(default)]
    body_md: String,
    #[serde(default = "default_state")]
    state: String,
    #[serde(default)]
    linked_tickets: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ObjectiveEditInput {
    id: String,
    title: Option<String>,
    old_string: Option<String>,
    new_string: Option<String>,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Debug, Serialize)]
struct ObjectiveEditRequest {
    title: Option<String>,
    old_string: Option<String>,
    new_string: Option<String>,
    replace_all: bool,
}

#[derive(Debug, Deserialize)]
struct ObjectiveSetStateInput {
    id: String,
    state: String,
}

#[derive(Debug, Serialize)]
struct ObjectiveSetStateRequest {
    state: String,
}

#[derive(Debug, Deserialize)]
struct ObjectiveLinkTicketInput {
    id: String,
    ticket_id: String,
}

#[derive(Debug, Deserialize)]
struct ObjectiveUnlinkTicketInput {
    id: String,
    ticket_id: String,
}

#[derive(Debug, Serialize)]
struct ObjectiveLinkTicketRequest {
    ticket_id: String,
}

fn default_state() -> String {
    "active".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agen::tool::ToolDefinition;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn tool_names(definitions: Vec<ToolDefinition>) -> Vec<String> {
        let mut names = definitions
            .into_iter()
            .map(|tool| tool().0.name)
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn workspace_http_objective_tools_include_objective_crud_tools() {
        let names = tool_names(workspace_http_objective_tools(Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace", "http://backend"),
        )));

        assert_eq!(
            names,
            vec![
                "ObjectiveCreate",
                "ObjectiveEdit",
                "ObjectiveLinkTicket",
                "ObjectiveSetState",
                "ObjectiveUnlinkTicket",
                "QueryObjective",
                "ShowObjective",
            ]
        );
    }

    #[test]
    fn objective_tool_schemas_are_bounded_and_mutation_scoped() {
        let list = list_schema();
        assert_eq!(list["properties"]["limit"]["maximum"], 100);
        assert!(list["properties"]["cursor"].is_object());
        assert!(list["properties"]["linked_ticket_id"].is_object());
        let show = show_schema();
        assert_eq!(show["required"][0], "id");
        assert_eq!(show["properties"]["event_limit"]["maximum"], 50);
        let create = create_schema();
        assert_eq!(create["required"][0], "title");
        let edit = edit_schema();
        assert_eq!(edit["required"][0], "id");
        let link = link_ticket_schema();
        assert_eq!(link["required"], json!(["id", "ticket_id"]));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn objective_show_summary_uses_projected_key() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let len = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(
                request.starts_with("POST /api/w/workspace/objectives/00001INTERNAL/show HTTP/1.1")
            );
            let body = serde_json::json!({
                "id": "00001INTERNAL",
                "resource_key": "O-3",
                "title": "Objective",
                "body": "Body",
                "body_truncated": false,
                "state": "active",
                "revision": "rev-1",
                "created_at": null,
                "updated_at": null,
                "linked_ticket_summaries": [],
                "events": [],
                "event_page": {"next_cursor": null, "has_more": false}
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let backend = WorkspaceHttpObjectiveBackend::new(Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace", base_url),
        ));

        let output = backend
            .show(ShowObjectiveInput {
                id: "00001INTERNAL".to_string(),
                event_limit: None,
                event_cursor: None,
            })
            .await
            .unwrap();

        server.join().unwrap();
        assert_eq!(output.summary, "Read objective O-3");
        assert!(!output.summary.contains("00001INTERNAL"));
        assert!(!output.content.unwrap().contains("00001INTERNAL"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn objective_link_summaries_resolve_internal_ticket_ids_to_keys() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for mutation in ["POST", "DELETE"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 8192];
                let len = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..len]);
                assert!(request.starts_with("GET /api/w/workspace/tickets/00001INTERNAL HTTP/1.1"));
                let response_body = serde_json::json!({"resource_key": "T-7"}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    response_body.len(),
                    response_body
                )
                .unwrap();

                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 8192];
                let len = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..len]);
                assert!(request.starts_with(&format!(
                    "{mutation} /api/w/workspace/objectives/O-3/ticket-links"
                )));
                let response_body = serde_json::json!({
                    "id": "00001OBJECTIVE",
                    "resource_key": "O-3",
                    "title": "Objective",
                    "body": "Body",
                    "body_truncated": false,
                    "state": "active",
                    "revision": format!("rev-{mutation}"),
                    "created_at": null,
                    "updated_at": null,
                    "linked_ticket_summaries": [],
                    "events": [],
                    "event_page": {"next_cursor": null, "has_more": false}
                })
                .to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    response_body.len(),
                    response_body
                )
                .unwrap();
            }
        });
        let backend = WorkspaceHttpObjectiveBackend::new(Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace", base_url),
        ));

        let linked = backend
            .link_ticket(ObjectiveLinkTicketInput {
                id: "O-3".to_string(),
                ticket_id: "00001INTERNAL".to_string(),
            })
            .await
            .unwrap();
        let unlinked = backend
            .unlink_ticket(ObjectiveUnlinkTicketInput {
                id: "O-3".to_string(),
                ticket_id: "00001INTERNAL".to_string(),
            })
            .await
            .unwrap();

        server.join().unwrap();
        for output in [linked, unlinked] {
            assert!(output.summary.contains("T-7"));
            assert!(!output.summary.contains("00001INTERNAL"));
            assert!(!output.content.unwrap().contains("00001INTERNAL"));
        }
    }

    #[test]
    fn native_objective_contract_is_route_bound_and_host_allocated() {
        let collection = objective_collection_descriptor();
        assert_eq!(
            collection
                .operations
                .iter()
                .map(|operation| operation.name.as_str())
                .collect::<Vec<_>>(),
            ["query", "create"]
        );
        let item = objective_item_descriptor();
        assert_eq!(
            item.operations
                .iter()
                .map(|operation| operation.name.as_str())
                .collect::<Vec<_>>(),
            ["read", "edit", "set_state", "link_ticket", "unlink_ticket"]
        );
        assert!(item.operations.iter().all(|operation| {
            operation
                .parameters
                .iter()
                .all(|parameter| parameter.name != "id")
        }));

        let mut registry = WipMountRegistry::new();
        let feature_route = registry.allocate_feature_route("objective").unwrap();
        mount_workspace_http_objective_wip(
            &mut registry,
            Arc::new(crate::worker::TestWorkspaceHttpClient::new(
                "workspace",
                "http://backend",
            )),
            None,
            &feature_route,
        )
        .unwrap();
        assert_eq!(
            registry.routes().collect::<Vec<_>>(),
            ["/features/objective/objectives"]
        );
        assert_eq!(
            feature_route.child("objectives").unwrap(),
            "/features/objective/objectives"
        );
        assert!(feature_route.child("../tickets").is_err());
    }

    #[test]
    fn dynamic_objective_routes_accept_only_objective_references_and_refresh_revision() {
        let revisions = Arc::new(Mutex::new(HashMap::new()));
        let resolver = ObjectiveItemResolver {
            backend: WorkspaceHttpObjectiveBackend::new(Arc::new(
                crate::worker::TestWorkspaceHttpClient::new("workspace", "http://backend"),
            )),
            permissions: None,
            collection_route: "/features/objective/objectives".into(),
            revisions: Arc::clone(&revisions),
        };
        let initial = resolver.resolve("O-3").unwrap();
        assert_eq!(initial.object.name, "O-3");
        assert!(resolver.resolve("00001OBJECTIVE").is_some());
        assert!(resolver.resolve("T-3").is_none());
        assert!(resolver.resolve("O-3/other").is_none());

        record_revision(
            &revisions,
            Some("O-3"),
            &json!({"objective": "O-3", "revision": "rev-2"}),
        );
        let refreshed = resolver.resolve("O-3").unwrap();
        assert_ne!(initial.object.validator, refreshed.object.validator);
    }

    #[tokio::test]
    async fn native_permissions_and_bounds_fail_before_backend_dispatch() {
        let backend = WorkspaceHttpObjectiveBackend::new(Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace", "http://unreachable.invalid"),
        ));
        let denied = ObjectiveItemWipHandler {
            backend: backend.clone(),
            permissions: Some(ToolPermissionConfig {
                default_action: ToolPermissionAction::Deny,
                rules: Vec::new(),
            }),
            objective_reference: "O-3".into(),
            revisions: Arc::new(Mutex::new(HashMap::new())),
        };
        let result = denied
            .call(
                "read",
                &BTreeMap::new(),
                WipCallContext {
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

        let collection = ObjectiveCollectionWipHandler {
            backend,
            permissions: None,
            collection_route: "/features/objective/objectives".into(),
            revisions: Arc::new(Mutex::new(HashMap::new())),
        };
        let result = collection
            .call(
                "query",
                &BTreeMap::from([("limit".into(), Value::Integer(101))]),
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "test".into(),
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(WipOperationError::Protocol(ProtocolError {
                code: ProtocolErrorCode::InvalidArguments,
                ..
            }))
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn native_query_preserves_cursor_and_returns_canonical_item_paths() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let len = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(request.starts_with("POST /api/w/workspace/objectives/query HTTP/1.1"));
            let response_body = serde_json::json!({
                "items": [{
                    "id": "00001OBJECTIVE",
                    "resource_key": "O-3",
                    "title": "Objective",
                    "state": "active",
                    "created_at": null,
                    "updated_at": null,
                    "matched_fields": ["title"],
                    "snippet": "Objective",
                    "linked_ticket_count": 1,
                    "linked_tickets": ["00001TICKET"],
                    "linked_ticket_keys": ["T-7"]
                }],
                "page": {"next_cursor": "next-page", "has_more": true},
                "record_authority": "workspace_sqlite"
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
        });
        let handler = ObjectiveCollectionWipHandler {
            backend: WorkspaceHttpObjectiveBackend::new(Arc::new(
                crate::worker::TestWorkspaceHttpClient::new("workspace", base_url),
            )),
            permissions: None,
            collection_route: "/features/objective/objectives".into(),
            revisions: Arc::new(Mutex::new(HashMap::new())),
        };
        let output = match handler
            .call(
                "query",
                &BTreeMap::new(),
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "test".into(),
                },
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("native query should succeed"),
        };
        server.join().unwrap();
        let output = crate::wip::wip_to_json(&output.value).unwrap();
        assert_eq!(output["next_cursor"], "next-page");
        assert_eq!(output["has_more"], true);
        assert_eq!(
            output["objectives"][0]["path"],
            "/features/objective/objectives/O-3"
        );
        assert_eq!(output["objectives"][0]["linked_tickets"][0], "T-7");
        assert!(!output.to_string().contains("00001OBJECTIVE"));
        assert!(!output.to_string().contains("00001TICKET"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn native_edit_binds_target_from_route_and_publishes_new_revision() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let len = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..len]);
            assert!(request.starts_with("PATCH /api/w/workspace/objectives/O-3 HTTP/1.1"));
            let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
            assert!(body.contains("Changed"));
            assert!(!body.contains("\"id\""));
            let response_body = serde_json::json!({
                "id": "00001OBJECTIVE",
                "resource_key": "O-3",
                "title": "Changed",
                "body": "Body",
                "body_truncated": false,
                "state": "active",
                "revision": "rev-2",
                "created_at": null,
                "updated_at": null,
                "linked_ticket_summaries": [],
                "events": [],
                "event_page": {"next_cursor": null, "has_more": false}
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            )
            .unwrap();
        });
        let revisions = Arc::new(Mutex::new(HashMap::new()));
        let backend = WorkspaceHttpObjectiveBackend::new(Arc::new(
            crate::worker::TestWorkspaceHttpClient::new("workspace", base_url),
        ));
        let resolver = ObjectiveItemResolver {
            backend: backend.clone(),
            permissions: None,
            collection_route: "/features/objective/objectives".into(),
            revisions: Arc::clone(&revisions),
        };
        let before = resolver.resolve("O-3").unwrap().object.validator;
        let handler = ObjectiveItemWipHandler {
            backend,
            permissions: None,
            objective_reference: "O-3".into(),
            revisions,
        };
        let output = match handler
            .call(
                "edit",
                &BTreeMap::from([("title".into(), Value::String("Changed".into()))]),
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "test".into(),
                },
            )
            .await
        {
            Ok(output) => output,
            Err(_) => panic!("native edit should succeed"),
        };
        server.join().unwrap();
        let output = crate::wip::wip_to_json(&output.value).unwrap();
        assert_eq!(output["objective"], "O-3");
        assert_eq!(output["revision"], "rev-2");
        let after = resolver.resolve("O-3").unwrap().object.validator;
        assert_ne!(before, after);
    }

    #[test]
    fn objective_output_rejects_noncanonical_keys() {
        let response = json!({
            "objective": "O-internal",
            "title": "Objective",
            "state": "active"
        });
        assert!(objective_output("created".to_string(), &response).is_err());
    }
}
