//!
//! Objective tool registration backed by Workspace API authority.
//!
//! Objectives are project-level planning context. Runtime Workers may not know
//! local `.yoi/objectives` paths, so model-visible Objective tools go through
//! the scoped Workspace API.

use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

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

    async fn list(&self, input: QueryObjectiveInput) -> Result<ToolOutput, ToolError> {
        let url = format!(
            "/api/w/{}/objectives/query",
            self.client.workspace_id().unwrap_or_default()
        );
        let response = send_json::<QueryObjectiveInput, serde_json::Value>(
            self.client.as_ref(),
            reqwest::Method::POST,
            &url,
            &input,
        )
        .await
        .map_err(backend_error)?;
        let response = project_objective_query(response).map_err(ToolError::ExecutionFailed)?;
        Ok(ToolOutput {
            summary: "Queried Objectives".to_string(),
            content: Some(serde_json::to_string_pretty(&response).map_err(decode_error)?),
            attachments: Vec::new(),
        })
    }

    async fn show(&self, input: ShowObjectiveInput) -> Result<ToolOutput, ToolError> {
        let id = validate_id(&input.id, "ShowObjective")?;
        let url = format!("{}/show", self.objective_url(id));
        let response = send_json::<ObjectiveShowRequest, serde_json::Value>(
            self.client.as_ref(),
            reqwest::Method::POST,
            &url,
            &ObjectiveShowRequest {
                event_limit: input.event_limit,
                event_cursor: input.event_cursor,
            },
        )
        .await
        .map_err(backend_error)?;
        let response = project_objective_detail(response).map_err(ToolError::ExecutionFailed)?;
        Ok(ToolOutput {
            summary: format!("Read objective {id}"),
            content: Some(serde_json::to_string_pretty(&response).map_err(decode_error)?),
            attachments: Vec::new(),
        })
    }

    async fn create(&self, input: ObjectiveCreateInput) -> Result<ToolOutput, ToolError> {
        if input.title.trim().is_empty() {
            return Err(ToolError::InvalidArgument(
                "ObjectiveCreate requires non-empty title".to_string(),
            ));
        }
        let url = format!(
            "/api/w/{}/objectives",
            self.client.workspace_id().unwrap_or_default()
        );
        let response = send_json::<ObjectiveCreateInput, ObjectiveDetail>(
            self.client.as_ref(),
            reqwest::Method::POST,
            &url,
            &input,
        )
        .await
        .map_err(backend_error)?;
        Ok(objective_output(
            format!("Created objective {}", &response.resource_key),
            response,
        )?)
    }

    async fn edit(&self, input: ObjectiveEditInput) -> Result<ToolOutput, ToolError> {
        let id = validate_id(&input.id, "ObjectiveEdit")?;
        if input.title.is_none() && input.old_string.is_none() && input.new_string.is_none() {
            return Err(ToolError::InvalidArgument(
                "ObjectiveEdit requires title or old_string/new_string".to_string(),
            ));
        }
        let url = self.objective_url(id);
        let body = ObjectiveEditRequest {
            title: input.title,
            old_string: input.old_string,
            new_string: input.new_string,
            replace_all: input.replace_all,
        };
        let response = send_json::<ObjectiveEditRequest, ObjectiveDetail>(
            self.client.as_ref(),
            reqwest::Method::PATCH,
            &url,
            &body,
        )
        .await
        .map_err(backend_error)?;
        Ok(objective_output(
            format!("Edited objective {}", &response.resource_key),
            response,
        )?)
    }

    async fn set_state(&self, input: ObjectiveSetStateInput) -> Result<ToolOutput, ToolError> {
        let id = validate_id(&input.id, "ObjectiveSetState")?;
        if input.state.trim().is_empty() {
            return Err(ToolError::InvalidArgument(
                "ObjectiveSetState requires non-empty state".to_string(),
            ));
        }
        let url = format!("{}/state", self.objective_url(id));
        let response = send_json::<ObjectiveSetStateRequest, ObjectiveDetail>(
            self.client.as_ref(),
            reqwest::Method::POST,
            &url,
            &ObjectiveSetStateRequest { state: input.state },
        )
        .await
        .map_err(backend_error)?;
        Ok(objective_output(
            format!("Updated objective {} state", &response.resource_key),
            response,
        )?)
    }

    async fn link_ticket(&self, input: ObjectiveLinkTicketInput) -> Result<ToolOutput, ToolError> {
        let id = validate_id(&input.id, "ObjectiveLinkTicket")?;
        let ticket_id = validate_id(&input.ticket_id, "ObjectiveLinkTicket")?;
        let url = format!("{}/ticket-links", self.objective_url(id));
        let response = send_json::<ObjectiveLinkTicketRequest, ObjectiveDetail>(
            self.client.as_ref(),
            reqwest::Method::POST,
            &url,
            &ObjectiveLinkTicketRequest {
                ticket_id: ticket_id.to_string(),
            },
        )
        .await
        .map_err(backend_error)?;
        Ok(objective_output(
            format!(
                "Linked ticket {ticket_id} to objective {}",
                &response.resource_key
            ),
            response,
        )?)
    }

    async fn unlink_ticket(
        &self,
        input: ObjectiveUnlinkTicketInput,
    ) -> Result<ToolOutput, ToolError> {
        let id = validate_id(&input.id, "ObjectiveUnlinkTicket")?;
        let ticket_id = validate_id(&input.ticket_id, "ObjectiveUnlinkTicket")?;
        let url = format!("{}/ticket-links/{}", self.objective_url(id), ticket_id);
        let response = delete_json::<ObjectiveDetail>(self.client.as_ref(), &url)
            .await
            .map_err(backend_error)?;
        Ok(objective_output(
            format!(
                "Unlinked ticket {ticket_id} from objective {}",
                &response.resource_key
            ),
            response,
        )?)
    }

    fn objective_url(&self, id: &str) -> String {
        let workspace_id = self.client.workspace_id().unwrap_or_default();
        format!("/api/w/{workspace_id}/objectives/{id}")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceObjectiveBackendError {
    #[error("workspace objective backend request failed: {0}")]
    Request(#[from] crate::worker::WorkspaceClientError),
    #[error("workspace objective backend returned HTTP {status}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("decode objective backend response: {0}")]
    Decode(#[from] serde_json::Error),
}

fn decode_error(error: serde_json::Error) -> ToolError {
    ToolError::ExecutionFailed(format!("decode objective backend response: {error}"))
}

fn backend_error(error: WorkspaceObjectiveBackendError) -> ToolError {
    ToolError::ExecutionFailed(error.to_string())
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

fn objective_output(summary: String, response: ObjectiveDetail) -> Result<ToolOutput, ToolError> {
    if !response.resource_key.starts_with("O-") {
        return Err(ToolError::ExecutionFailed(
            "required O- human key is unavailable".to_string(),
        ));
    }
    let projected = serde_json::json!({
        "objective": &response.resource_key,
        "title": response.title,
        "state": response.state,
    });
    Ok(ToolOutput {
        summary,
        content: Some(serde_json::to_string_pretty(&projected).map_err(decode_error)?),

        attachments: Vec::new(),
    })
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

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct ObjectiveDetail {
    resource_key: String,
    title: String,
    state: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use agen::tool::ToolDefinition;

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
}
