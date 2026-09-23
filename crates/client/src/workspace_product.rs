use server_api::{
    BrowserCreateWorkerResponse, BrowserWorkspaceOrchestratorResponse,
    CreateWorkspaceWorkerRequest, ListResponse, MemoryDocumentResponse, MemoryStagingListResponse,
    ObjectiveCreateRequest, ObjectiveDetail, ObjectiveEditRequest, ObjectiveLinkTicketRequest,
    ObjectiveStateRequest, ObjectiveSummary, RevokeRuntimeTrustKeyRequest,
    RuntimeTrustKeyRevealResponse, WorkerLaunchOptionsResponse, WorkspaceRuntimeDetail,
    WorkspaceRuntimeResource,
};
use ticket::{
    MarkdownText, NewOrchestrationPlanRecord, NewTicket, NewTicketEvent, NewTicketRelation,
    OrchestrationPlanKind, OrchestrationPlanRecord, Ticket, TicketBackend, TicketDependencyCheck,
    TicketDoctorReport, TicketError, TicketIdOrSlug, TicketIntakeSummary, TicketItemEdit,
    TicketListQuery, TicketListState, TicketMarkReady, TicketRef, TicketRelation,
    TicketRelationKind, TicketRelationView, TicketStateChange, TicketStateSelector, TicketSummary,
};

use crate::{BackendApiClient, BackendWorkspaceClientError};

const DEFAULT_PRODUCT_LIST_LIMIT: usize = 1_000;

/// Workspace-scoped Backend client for Ticket and Objective product state.
///
/// Construction requires both the selected Backend URL and Workspace identity.
/// Callers should derive these once from `Target::resolve()` and must not retry
/// failed requests against repository-local state.
#[derive(Debug, Clone)]
pub struct BackendWorkspaceProductClient {
    api: BackendApiClient,
    workspace_id: String,
}

impl BackendWorkspaceProductClient {
    pub fn new(
        base_url: impl Into<String>,
        workspace_id: impl Into<String>,
    ) -> Result<Self, BackendWorkspaceClientError> {
        let base_url = base_url.into();
        let api = BackendApiClient::from_stored_token(&base_url)?;
        let workspace_id = workspace_id.into();
        if workspace_id.trim().is_empty() {
            return Err(BackendWorkspaceClientError::InvalidTarget(
                "Backend Workspace identity must not be empty".into(),
            ));
        }
        Ok(Self { api, workspace_id })
    }

    #[cfg(test)]
    fn new_with_access_token(
        base_url: impl Into<String>,
        workspace_id: impl Into<String>,
        access_token: &str,
    ) -> Result<Self, BackendWorkspaceClientError> {
        let base_url = base_url.into();
        let api = BackendApiClient::from_access_token_for_test(&base_url, access_token)?;
        let workspace_id = workspace_id.into();
        if workspace_id.trim().is_empty() {
            return Err(BackendWorkspaceClientError::InvalidTarget(
                "Backend Workspace identity must not be empty".into(),
            ));
        }
        Ok(Self { api, workspace_id })
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    fn generated<T, F, Fut>(&self, operation: F) -> Result<T, BackendWorkspaceClientError>
    where
        F: FnOnce(
            server_api::ServerApiClient<crate::backend_workspace::ServerBearerAuthorizer>,
        ) -> Fut,
        Fut: std::future::Future<
                Output = Result<
                    T,
                    server_api::client_support::ClientError<server_api::RepositoryApiError>,
                >,
            >,
    {
        let client = crate::backend_workspace::server_api_client(&self.api)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| BackendWorkspaceClientError::InvalidTarget(error.to_string()))?;
        runtime
            .block_on(operation(client))
            .map_err(|error| crate::backend_workspace::server_client_error(&self.api, error))
    }

    pub fn list_tickets(
        &self,
        query: &TicketListQuery,
    ) -> Result<Vec<TicketSummary>, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let state = ticket_list_state_query(query);
        self.generated(move |client| async move {
            client
                .ticket_summary_search(
                    workspace_id,
                    server_api::TicketSummarySearchQuery {
                        state: Some(state),
                        limit: None,
                    },
                )
                .await
                .map(|response| response.0)
        })
    }

    pub fn show_ticket(&self, id: &TicketIdOrSlug) -> Result<Ticket, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(id);
        self.generated(move |client| async move {
            client
                .ticket_record_get(workspace_id, id)
                .await
                .map(|response| response.0)
        })
    }

    pub fn create_ticket(
        &self,
        input: &NewTicket,
    ) -> Result<TicketRef, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let input = input.clone();
        self.generated(move |client| async move {
            client
                .ticket_create_record(workspace_id, server_api::CreateTicketRecordRequest(input))
                .await
                .map(|response| response.0)
        })
    }

    pub fn add_ticket_event(
        &self,
        id: &TicketIdOrSlug,
        event: &NewTicketEvent,
    ) -> Result<(), BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(id);
        let event = event.clone();
        self.generated(move |client| async move {
            client
                .ticket_thread_event_add(
                    workspace_id,
                    id,
                    server_api::TicketThreadEventRequest(event),
                )
                .await
        })
    }

    pub fn set_ticket_workflow_state(
        &self,
        id: &TicketIdOrSlug,
        change: &TicketStateChange,
    ) -> Result<(), BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(id);
        let change = change.clone();
        self.generated(move |client| async move {
            client
                .ticket_workflow_state_set(
                    workspace_id,
                    id,
                    server_api::TicketStateChangeRequest(change),
                )
                .await
        })
    }

    pub fn close_ticket(
        &self,
        id: &TicketIdOrSlug,
        resolution: &MarkdownText,
    ) -> Result<(), BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(id);
        let resolution = resolution.clone();
        self.generated(move |client| async move {
            client
                .ticket_close_record(
                    workspace_id,
                    id,
                    server_api::TicketCloseRecordRequest(resolution),
                )
                .await
        })
    }

    pub fn add_ticket_relation(
        &self,
        id: &TicketIdOrSlug,
        relation: &NewTicketRelation,
    ) -> Result<TicketRelation, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(id);
        let relation = relation.clone();
        self.generated(move |client| async move {
            client
                .ticket_relation_record(
                    workspace_id,
                    id,
                    server_api::CreateTicketRelationRequest(relation),
                )
                .await
                .map(|response| response.0)
        })
    }

    pub fn query_ticket_relations(
        &self,
        ticket: Option<&TicketIdOrSlug>,
        kind: Option<TicketRelationKind>,
    ) -> Result<Vec<TicketRelation>, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let ticket = ticket.cloned();
        self.generated(move |client| async move {
            client
                .ticket_relation_query(
                    workspace_id,
                    server_api::TicketRelationSearchRequest { ticket, kind },
                )
                .await
                .map(|response| response.0)
        })
    }

    pub fn ticket_doctor(&self) -> Result<TicketDoctorReport, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        self.generated(move |client| async move {
            client
                .ticket_doctor(workspace_id)
                .await
                .map(|response| response.0)
        })
    }

    pub fn list_objectives(
        &self,
        limit: usize,
    ) -> Result<ListResponse<ObjectiveSummary>, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let response = self.generated(move |client| async move {
            client
                .objective_list(
                    workspace_id,
                    server_api::ObjectiveListQuery { limit: Some(limit) },
                )
                .await
        })?;
        Ok(ListResponse {
            workspace_id: response.workspace_id,
            limit: response.limit,
            items: response.items,
            source: response.record_authority,
            diagnostics: response
                .invalid_records
                .into_iter()
                .map(|invalid| server_api::Diagnostic {
                    code: "invalid_objective_record".to_string(),
                    severity: server_api::DiagnosticSeverity::Warning,
                    message: format!("{}: {}", invalid.label, invalid.reason),
                })
                .collect(),
        })
    }

    pub fn show_objective(&self, id: &str) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let id = id.to_string();
        self.generated(move |client| async move { client.objective_get(workspace_id, id).await })
    }

    pub fn create_objective(
        &self,
        input: &ObjectiveCreateRequest,
    ) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let input = input.clone();
        self.generated(
            move |client| async move { client.objective_create(workspace_id, input).await },
        )
    }

    pub fn edit_objective(
        &self,
        id: &str,
        input: &ObjectiveEditRequest,
    ) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let id = id.to_string();
        let input = input.clone();
        self.generated(
            move |client| async move { client.objective_edit(workspace_id, id, input).await },
        )
    }

    pub fn set_objective_state(
        &self,
        id: &str,
        input: &ObjectiveStateRequest,
    ) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let id = id.to_string();
        let input = input.clone();
        self.generated(move |client| async move {
            client.objective_state_set(workspace_id, id, input).await
        })
    }

    pub fn link_objective_ticket(
        &self,
        id: &str,
        input: &ObjectiveLinkTicketRequest,
    ) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let id = id.to_string();
        let input = input.clone();
        self.generated(move |client| async move {
            client.objective_ticket_link(workspace_id, id, input).await
        })
    }

    pub fn unlink_objective_ticket(
        &self,
        id: &str,
        ticket_id: &str,
    ) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let id = id.to_string();
        let ticket_id = ticket_id.to_string();
        self.generated(move |client| async move {
            client
                .objective_ticket_unlink(workspace_id, id, ticket_id)
                .await
        })
    }

    pub fn list_runtimes(
        &self,
    ) -> Result<ListResponse<WorkspaceRuntimeResource>, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let response =
            self.generated(move |client| async move { client.runtime_list(workspace_id).await })?;
        Ok(ListResponse {
            workspace_id: response.workspace_id,
            limit: response.limit,
            items: response.items,
            source: response.source,
            diagnostics: response.diagnostics,
        })
    }

    pub fn runtime_detail(
        &self,
        runtime_id: &str,
    ) -> Result<WorkspaceRuntimeDetail, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let runtime_id = runtime_id.to_string();
        self.generated(move |client| async move {
            client.runtime_detail(workspace_id, runtime_id).await
        })
    }

    pub fn reveal_runtime_trust_key(
        &self,
        runtime_id: &str,
    ) -> Result<RuntimeTrustKeyRevealResponse, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let runtime_id = runtime_id.to_string();
        self.generated(move |client| async move {
            client
                .runtime_trust_key_reveal(workspace_id, runtime_id)
                .await
        })
    }

    pub fn revoke_runtime_trust_key(
        &self,
        runtime_id: &str,
        request: &RevokeRuntimeTrustKeyRequest,
    ) -> Result<WorkspaceRuntimeDetail, BackendWorkspaceClientError> {
        let client = crate::backend_workspace::server_api_client(&self.api)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| BackendWorkspaceClientError::InvalidTarget(error.to_string()))?;
        runtime
            .block_on(client.runtime_trust_key_revoke(
                self.workspace_id.clone(),
                runtime_id.to_string(),
                request.clone(),
            ))
            .map_err(|error| {
                crate::backend_workspace::runtime_management_client_error(&self.api, error)
            })
    }

    pub fn memory_document(&self) -> Result<MemoryDocumentResponse, BackendWorkspaceClientError> {
        crate::backend_workspace::memory_document_blocking(&self.api, &self.workspace_id)
    }

    pub fn list_memory_staging(
        &self,
        limit: usize,
    ) -> Result<MemoryStagingListResponse, BackendWorkspaceClientError> {
        crate::backend_workspace::memory_staging_list_blocking(&self.api, &self.workspace_id, limit)
    }

    pub fn launch_ticket_intake(
        &self,
        ticket_id: &str,
    ) -> Result<String, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let options: WorkerLaunchOptionsResponse = self.generated(move |client| async move {
            client.workspace_worker_launch_options(workspace_id).await
        })?;
        let runtime = options
            .runtimes
            .iter()
            .find(|runtime| runtime.worker_creation_available && !runtime.working_directory_required)
            .ok_or_else(|| {
                BackendWorkspaceClientError::InvalidTarget(
                    "Backend has no spawn-capable Runtime that supports a Workdir-less Intake Worker"
                        .to_string(),
                )
            })?;
        let request = CreateWorkspaceWorkerRequest {
            runtime_id: runtime.runtime_id.clone(),
            display_name: format!("intake-{ticket_id}"),
            profile: Some("builtin:intake".to_string()),
            ticket_assignment: None,
            initial_submit: vec![protocol::Segment::Text {
                content: format!("Please handle intake for Ticket {ticket_id}."),
            }],
            workdir_attachments: Vec::new(),
            control_operation_id: None,
        };
        let workspace_id = self.workspace_id.clone();
        let response: BrowserCreateWorkerResponse = self.generated(move |client| async move {
            client.workspace_worker_create(workspace_id, request).await
        })?;
        Ok(format!(
            "Started Intake Worker {}/{} for Ticket {ticket_id}",
            response.runtime_id, response.worker_id
        ))
    }

    pub fn start_workspace_orchestrator(&self) -> Result<String, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let response: BrowserWorkspaceOrchestratorResponse =
            self.generated(move |client| async move {
                client.workspace_orchestrator_start(workspace_id).await
            })?;
        let worker = response.worker.ok_or_else(|| {
            BackendWorkspaceClientError::InvalidTarget(
                "Backend accepted the Orchestrator request without returning a Worker".to_string(),
            )
        })?;
        Ok(format!(
            "Workspace Orchestrator {} at {}/{}",
            response.disposition, worker.runtime_id, worker.worker_id
        ))
    }

    pub fn default_product_list_limit() -> usize {
        DEFAULT_PRODUCT_LIST_LIMIT
    }
}

impl TicketBackend for BackendWorkspaceProductClient {
    fn default_intake_ready_state_change_body(&self, from: &str) -> String {
        let workspace_id = self.workspace_id.clone();
        let from = from.to_string();
        self.generated(move |client| async move {
            client
                .ticket_default_intake_ready_body(
                    workspace_id,
                    server_api::DefaultIntakeReadyBodyRequest { from },
                )
                .await
                .map(|response| response.0)
        })
        .unwrap_or_else(|error| error.to_string())
    }

    fn list(&self, filter: TicketListQuery) -> ticket::Result<Vec<TicketSummary>> {
        self.list_tickets(&filter).map_err(ticket_client_error)
    }

    fn show(&self, id: TicketIdOrSlug) -> ticket::Result<Ticket> {
        self.show_ticket(&id).map_err(ticket_client_error)
    }

    fn create(&self, input: NewTicket) -> ticket::Result<TicketRef> {
        self.create_ticket(&input).map_err(ticket_client_error)
    }

    fn edit_item(&self, id: TicketIdOrSlug, edit: TicketItemEdit) -> ticket::Result<Ticket> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(&id);
        self.generated(move |client| async move {
            client
                .ticket_record_item_edit(
                    workspace_id,
                    id,
                    server_api::EditTicketRecordItemRequest(edit),
                )
                .await
                .map(|response| response.0)
        })
        .map_err(ticket_client_error)
    }

    fn dependency_check(&self, id: TicketIdOrSlug) -> ticket::Result<TicketDependencyCheck> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(&id);
        self.generated(move |client| async move {
            client
                .ticket_dependency_check(workspace_id, id)
                .await
                .map(|response| response.0)
        })
        .map_err(ticket_client_error)
    }

    fn add_event(&self, id: TicketIdOrSlug, event: NewTicketEvent) -> ticket::Result<()> {
        self.add_ticket_event(&id, &event)
            .map_err(ticket_client_error)
    }

    fn add_state_changed(
        &self,
        id: TicketIdOrSlug,
        change: TicketStateChange,
    ) -> ticket::Result<()> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(&id);
        self.generated(move |client| async move {
            client
                .ticket_state_change_add(
                    workspace_id,
                    id,
                    server_api::TicketStateChangeRequest(change),
                )
                .await
        })
        .map_err(ticket_client_error)
    }

    fn add_intake_summary(
        &self,
        id: TicketIdOrSlug,
        summary: TicketIntakeSummary,
    ) -> ticket::Result<()> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(&id);
        self.generated(move |client| async move {
            client
                .ticket_intake_summary_add(
                    workspace_id,
                    id,
                    server_api::TicketIntakeSummaryRequest(summary),
                )
                .await
        })
        .map_err(ticket_client_error)
    }

    fn set_state_field(
        &self,
        id: TicketIdOrSlug,
        field: &str,
        change: TicketStateChange,
    ) -> ticket::Result<()> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(&id);
        let field = field.to_string();
        self.generated(move |client| async move {
            client
                .ticket_state_field_set(
                    workspace_id,
                    id,
                    field,
                    server_api::TicketStateChangeRequest(change),
                )
                .await
        })
        .map_err(ticket_client_error)
    }

    fn set_workflow_state(
        &self,
        id: TicketIdOrSlug,
        change: TicketStateChange,
    ) -> ticket::Result<()> {
        self.set_ticket_workflow_state(&id, &change)
            .map_err(ticket_client_error)
    }

    fn mark_ready(&self, id: TicketIdOrSlug, request: TicketMarkReady) -> ticket::Result<Ticket> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(&id);
        let request = server_api::TicketMarkReadyRequest {
            operation_key: request.operation_key,
            reason: request.reason,
            intake_summary: request.intake_summary,
        };
        self.generated(move |client| async move {
            client
                .ticket_mark_ready_record(workspace_id, id, request)
                .await
                .map(|response| response.0)
        })
        .map_err(ticket_client_error)
    }

    fn queue_ready(
        &self,
        id: TicketIdOrSlug,
        _queued_by: &str,
    ) -> ticket::Result<ticket::TicketQueueOutcome> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(&id);
        self.generated(move |client| async move {
            client
                .ticket_queue_record(workspace_id, id)
                .await
                .map(|response| response.0)
        })
        .map_err(ticket_client_error)
    }

    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> ticket::Result<()> {
        self.close_ticket(&id, &resolution)
            .map_err(ticket_client_error)
    }

    fn add_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        relation: NewTicketRelation,
    ) -> ticket::Result<TicketRelation> {
        BackendWorkspaceProductClient::add_ticket_relation(self, &id, &relation)
            .map_err(ticket_client_error)
    }

    fn remove_ticket_relation(
        &self,
        id: TicketIdOrSlug,
        kind: TicketRelationKind,
        target: TicketIdOrSlug,
    ) -> ticket::Result<TicketRelation> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(&id);
        let target = ticket_reference(&target);
        self.generated(move |client| async move {
            client
                .ticket_relation_remove(
                    workspace_id,
                    id,
                    server_api::TicketRelationRemoveRequest { kind, target },
                )
                .await
                .map(|response| response.0)
        })
        .map_err(ticket_client_error)
    }

    fn query_ticket_relations(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<TicketRelationKind>,
    ) -> ticket::Result<Vec<TicketRelation>> {
        BackendWorkspaceProductClient::query_ticket_relations(self, ticket.as_ref(), kind)
            .map_err(ticket_client_error)
    }

    fn relation_view(&self, id: TicketIdOrSlug) -> ticket::Result<TicketRelationView> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(&id);
        self.generated(move |client| async move {
            client
                .ticket_relation_view(workspace_id, id)
                .await
                .map(|response| response.0)
        })
        .map_err(ticket_client_error)
    }

    fn add_orchestration_plan_record(
        &self,
        id: TicketIdOrSlug,
        record: NewOrchestrationPlanRecord,
    ) -> ticket::Result<OrchestrationPlanRecord> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(&id);
        self.generated(move |client| async move {
            client
                .ticket_orchestration_plan_record(
                    workspace_id,
                    id,
                    server_api::CreateTicketOrchestrationPlanRequest(record),
                )
                .await
                .map(|response| response.0)
        })
        .map_err(ticket_client_error)
    }

    fn query_orchestration_plan_records(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<OrchestrationPlanKind>,
    ) -> ticket::Result<Vec<OrchestrationPlanRecord>> {
        let workspace_id = self.workspace_id.clone();
        self.generated(move |client| async move {
            client
                .ticket_orchestration_plan_query(
                    workspace_id,
                    server_api::TicketOrchestrationPlanSearchRequest { ticket, kind },
                )
                .await
                .map(|response| response.0)
        })
        .map_err(ticket_client_error)
    }

    fn doctor(&self) -> ticket::Result<TicketDoctorReport> {
        self.ticket_doctor().map_err(ticket_client_error)
    }
}
fn ticket_client_error(error: BackendWorkspaceClientError) -> TicketError {
    TicketError::Sqlite(format!("Backend request failed: {error}"))
}

fn ticket_reference(id: &TicketIdOrSlug) -> String {
    match id {
        TicketIdOrSlug::Id(id) => id.to_string(),
        TicketIdOrSlug::Slug(slug) | TicketIdOrSlug::Query(slug) => slug.clone(),
    }
}

fn ticket_list_state_query(query: &TicketListQuery) -> String {
    match &query.state {
        TicketStateSelector::Active => "active".to_string(),
        TicketStateSelector::All => "all".to_string(),
        TicketStateSelector::States(states) => states
            .iter()
            .copied()
            .map(TicketListState::as_str)
            .collect::<Vec<_>>()
            .join(","),
    }
}

#[cfg(test)]
fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            write!(&mut encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    fn one_response_server(
        status: &str,
        body: &str,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_string();
        let body = body.to_string();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = vec![0_u8; 8_192];
            let bytes = stream.read(&mut request).unwrap();
            sender
                .send(String::from_utf8_lossy(&request[..bytes]).to_string())
                .unwrap();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        (format!("http://{address}"), receiver, handle)
    }

    fn response_sequence_server(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = vec![0_u8; 16_384];
                let bytes = stream.read(&mut request).unwrap();
                sender
                    .send(String::from_utf8_lossy(&request[..bytes]).to_string())
                    .unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (format!("http://{address}"), receiver, handle)
    }

    #[test]
    fn memory_document_uses_shared_workspace_scoped_response() {
        let body = r##"{"body_md":"# Memory\\n","created_at":"2026-09-01T00:00:00Z","updated_at":"2026-09-02T00:00:00Z","bytes":10,"record_source":"workspace-sqlite"}"##;
        let (base_url, request, handle) = one_response_server("200 OK", body);
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-backend-token",
        )
        .unwrap();

        let response = client.memory_document().unwrap();

        assert_eq!(response.record_source, "workspace-sqlite");
        assert!(
            request
                .recv()
                .unwrap()
                .starts_with("GET /api/w/workspace-a/memory ")
        );
        handle.join().unwrap();
    }

    #[test]
    fn memory_staging_uses_shared_dto_with_typed_origin() {
        let body = r#"{"limit":10,"returned_count":1,"total_valid_count":1,"invalid_count":0,"truncated":false,"order":"imported_at_desc_candidate_id_asc","record_authority":"sqlite_workspace_authority.memory_staging","items":[{"id":"candidate-1","byte_len":128,"record":{"schema_version":1,"id":"candidate-1","extract_run_id":"run-1","source":{"segment_id":"segment-1","range":[1,2]},"kind":"decision","claim":"Keep typed provenance.","why_useful":"Prevents trust loss.","staleness":null,"evidence":[],"source_refs":[{"session_id":"session-1","segment_id":"segment-1","entry_range":[1,2],"evidence_id":"evidence-1","origin":{"kind":"worker_input","workspace_id":"workspace-a","runtime_id":"runtime-1","worker_id":"worker-1"},"evidence_kind":"worker_session_entry","label":null,"summary":null}]}}],"diagnostics":[]}"#;
        let (base_url, request, handle) = one_response_server("200 OK", body);
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-backend-token",
        )
        .unwrap();

        let response = client.list_memory_staging(10).unwrap();

        assert_eq!(
            response.items[0].record.source_refs[0]
                .origin
                .as_ref()
                .unwrap()
                .kind,
            server_api::MemoryEvidenceOriginKind::WorkerInput
        );
        assert!(
            request
                .recv()
                .unwrap()
                .starts_with("GET /api/w/workspace-a/memory/staging?limit=10 ")
        );
        handle.join().unwrap();
    }

    #[test]
    fn memory_staging_rejects_unknown_origin_kind() {
        let body = r#"{"limit":10,"returned_count":1,"total_valid_count":1,"invalid_count":0,"truncated":false,"order":"order","record_authority":"authority","items":[{"id":"candidate-1","byte_len":1,"record":{"schema_version":1,"id":"candidate-1","extract_run_id":"run-1","source":{"segment_id":"segment-1","range":[1,2]},"kind":"decision","claim":"claim","why_useful":"useful","staleness":null,"evidence":[],"source_refs":[{"session_id":null,"segment_id":null,"entry_range":null,"evidence_id":null,"origin":{"kind":"future_origin"},"evidence_kind":null,"label":null,"summary":null}]}}],"diagnostics":[]}"#;
        let (base_url, request, handle) = one_response_server("200 OK", body);
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-backend-token",
        )
        .unwrap();

        let error = client.list_memory_staging(10).unwrap_err();

        assert!(matches!(
            error,
            BackendWorkspaceClientError::ServerApi(
                server_api::client_support::ClientError::Failure(
                    server_api::client_support::ClientFailure::Decode {
                        kind: server_api::client_support::DecodeKind::Success,
                    },
                ),
            )
        ));
        assert!(
            request
                .recv()
                .unwrap()
                .starts_with("GET /api/w/workspace-a/memory/staging?limit=10 ")
        );
        handle.join().unwrap();
    }

    #[test]
    fn objective_list_uses_workspace_scoped_backend_route() {
        let body = r#"{"workspace_id":"workspace-a","limit":1000,"items":[],"invalid_records":[{"label":"broken.md","reason":"invalid frontmatter"}],"record_authority":"sqlite"}"#;
        let (base_url, request, handle) = one_response_server("200 OK", body);
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-backend-token",
        )
        .unwrap();

        let response = client.list_objectives(1_000).unwrap();

        assert!(response.items.is_empty());
        assert_eq!(response.source, "sqlite");
        assert_eq!(response.diagnostics.len(), 1);
        assert_eq!(response.diagnostics[0].code, "invalid_objective_record");
        assert_eq!(
            response.diagnostics[0].message,
            "broken.md: invalid frontmatter"
        );
        let request = request.recv().unwrap();
        assert!(request.starts_with("GET /api/w/workspace-a/objectives?limit=1000 "));
        assert!(request.contains("authorization: Bearer test-backend-token\r\n"));
        handle.join().unwrap();
    }

    #[test]
    fn backend_mutation_failure_is_returned_without_local_fallback() {
        let (base_url, request, handle) =
            one_response_server("403 Forbidden", "test-backend-token");
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-backend-token",
        )
        .unwrap();

        let error = client
            .create_objective(&ObjectiveCreateRequest {
                title: "Objective".to_string(),
                body_md: "body".to_string(),
                state: "active".to_string(),
                linked_tickets: Vec::new(),
            })
            .unwrap_err();

        assert!(error.to_string().contains("403"));
        assert!(!error.to_string().contains("test-backend-token"));
        assert!(
            request
                .recv()
                .unwrap()
                .starts_with("POST /api/w/workspace-a/objectives ")
        );
        handle.join().unwrap();
    }

    #[test]
    fn ticket_relation_query_uses_workspace_scoped_backend_route() {
        let (base_url, request, handle) = one_response_server("200 OK", "[]");
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-backend-token",
        )
        .unwrap();

        let relations = client
            .query_ticket_relations(
                Some(&TicketIdOrSlug::Query("T-1".to_string())),
                Some(TicketRelationKind::Related),
            )
            .unwrap();

        assert!(relations.is_empty());
        let request = request.recv().unwrap();
        assert!(request.starts_with("POST /api/w/workspace-a/tickets/relations/search "));
        assert!(request.contains("\"ticket\":{\"Query\":\"T-1\"}"));
        handle.join().unwrap();
    }

    #[test]
    fn orchestration_plan_query_uses_workspace_scoped_backend_route() {
        let (base_url, request, handle) = one_response_server("200 OK", "[]");
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-backend-token",
        )
        .unwrap();

        let records = TicketBackend::query_orchestration_plan_records(&client, None, None).unwrap();

        assert!(records.is_empty());
        assert!(
            request
                .recv()
                .unwrap()
                .starts_with("POST /api/w/workspace-a/tickets/orchestration-plans/search ")
        );
        handle.join().unwrap();
    }

    #[test]
    fn ticket_intake_launch_uses_backend_options_and_workspace_worker_route() {
        let (base_url, requests, handle) = response_sequence_server(vec![
            (
                "200 OK",
                r#"{"workspace_id":"workspace-a","runtimes":[{"runtime_id":"embedded","display_name":"Embedded","built_in":true,"worker_creation_available":true,"working_directory_required":false,"status":"connected","diagnostics":[]}],"default_profile":null,"profiles":[],"repositories":[],"working_directories":[],"diagnostics":[]}"#,
            ),
            (
                "200 OK",
                r#"{"workspace_id":"workspace-a","runtime_id":"embedded","worker_id":"worker-1","console_href":"/w/workspace-a/workers/worker-1","worker":{"runtime_id":"embedded","worker_id":"worker-1","host_id":"embedded","display_name":"Intake","label":"worker-1","profile":"builtin:intake","singleton_key":null,"tags":[],"workspace":{"visibility":"workspace","identity":"workspace-a","workspace_id":"workspace-a"},"state":"idle","last_seen_at":null,"pinned":false,"retention_state":"active","implementation":{"kind":"runtime","display_hint":"Runtime Worker"},"capabilities":{"can_stop":true,"can_spawn_followup":false},"diagnostics":[]},"diagnostics":[]}"#,
            ),
        ]);
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-backend-token",
        )
        .unwrap();

        let status = client.launch_ticket_intake("T-1").unwrap();

        assert!(status.contains("embedded/worker-1"));
        assert!(
            requests
                .recv()
                .unwrap()
                .starts_with("GET /api/w/workspace-a/workers/launch-options ")
        );
        let create_request = requests.recv().unwrap();
        assert!(create_request.starts_with("POST /api/w/workspace-a/workers "));
        assert!(create_request.contains("\"profile\":\"builtin:intake\""));
        assert!(create_request.contains("Ticket T-1"));
        handle.join().unwrap();
    }

    #[test]
    fn workspace_orchestrator_launch_uses_scoped_backend_route() {
        let body = r#"{"workspace_id":"workspace-a","online":true,"disposition":"created","worker":{"runtime_id":"embedded","worker_id":"worker-2","host_id":"embedded","display_name":"Orchestrator","label":"worker-2","profile":"builtin:orchestrator","singleton_key":"workspace-orchestrator","tags":[],"workspace":{"visibility":"workspace","identity":"workspace-a","workspace_id":"workspace-a"},"state":"idle","last_seen_at":null,"pinned":true,"retention_state":"active","implementation":{"kind":"runtime","display_hint":"Runtime Worker"},"capabilities":{"can_stop":true,"can_spawn_followup":false},"diagnostics":[]},"diagnostics":[]}"#;
        let (base_url, request, handle) = one_response_server("200 OK", body);
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-backend-token",
        )
        .unwrap();

        let status = client.start_workspace_orchestrator().unwrap();

        assert!(status.contains("created at embedded/worker-2"));
        assert!(
            request
                .recv()
                .unwrap()
                .starts_with("POST /api/w/workspace-a/orchestrator ")
        );
        handle.join().unwrap();
    }

    #[test]
    fn product_client_requires_workspace_identity() {
        let error = BackendWorkspaceProductClient::new_with_access_token(
            "http://127.0.0.1:8787",
            "",
            "test-backend-token",
        )
        .unwrap_err();
        assert!(error.to_string().contains("Workspace identity"));
    }

    #[test]
    fn ticket_state_query_preserves_local_filter_semantics() {
        assert_eq!(
            ticket_list_state_query(&TicketListQuery::active()),
            "active"
        );
        assert_eq!(ticket_list_state_query(&TicketListQuery::all()), "all");
        assert_eq!(
            ticket_list_state_query(&TicketListQuery {
                state: TicketStateSelector::States(
                    [TicketListState::Ready, TicketListState::InProgress]
                        .into_iter()
                        .collect(),
                ),
            }),
            "ready,inprogress"
        );
    }

    #[test]
    fn ticket_and_objective_references_are_path_encoded() {
        assert_eq!(encode_path_segment("T-1/a"), "T-1%2Fa");
    }
}
