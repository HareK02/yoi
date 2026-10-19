use server_api::{
    BrowserCreateWorkerResponse, BrowserWorkspaceOrchestratorResponse,
    CreateWorkspaceWorkerRequest, ListResponse, MemoryDocumentResponse, MemoryStagingListResponse,
    ObjectiveCreateRequest, ObjectiveDetail, ObjectiveEditRequest, ObjectiveLinkTicketRequest,
    ObjectiveStateRequest, ObjectiveSummary, RevokeRuntimeTrustKeyRequest,
    RuntimeTrustKeyRevealResponse, SubjektivMemoryChangesQuery, SubjektivMemoryDetailQuery,
    SubjektivMemoryListChangesResponse, SubjektivMemoryListQuery, SubjektivMemoryQueryResponse,
    SubjektivMemoryReadResponse, SubjektivResidentSurfaceResponse, SubjektivSubjectListQuery,
    SubjektivSubjectListResponse, SubjektivSubjectResponse, WorkerLaunchOptionsResponse,
    WorkspaceRuntimeDetail, WorkspaceRuntimeResource,
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
const UNSUPPORTED_TICKET_STATE_CHANGE: &str = "unsupported legacy Ticket state/close operation; use TicketBackend::update_case with caller-supplied content digest, case state, closure operation key, and replay key (CAS); case updates do not start or stop work";

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

    pub fn list_workspace_tags(
        &self,
        request: &ticket::TagListRequest,
    ) -> Result<Vec<ticket::WorkspaceTag>, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let request = request.clone();
        self.generated(move |client| async move {
            client
                .workspace_ticket_tag_list(
                    workspace_id,
                    server_api::WorkspaceTicketTagListRequest(request),
                )
                .await
                .map(|response| response.0)
        })
    }

    pub fn mutate_workspace_tag(
        &self,
        request: &ticket::WorkspaceTagMutation,
    ) -> Result<ticket::WorkspaceTag, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let request = request.clone();
        self.generated(move |client| async move {
            client
                .workspace_ticket_tag_mutate(
                    workspace_id,
                    server_api::WorkspaceTicketTagMutationRequest(request),
                )
                .await
                .map(|response| response.0)
        })
    }

    pub fn set_ticket_tags(
        &self,
        id: &TicketIdOrSlug,
        request: &ticket::TicketTagSetRequest,
    ) -> Result<Vec<ticket::WorkspaceTag>, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(id);
        let request = request.clone();
        self.generated(move |client| async move {
            client
                .ticket_tags_set(workspace_id, id, server_api::TicketTagsSetRequest(request))
                .await
                .map(|response| response.0)
        })
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
        _id: &TicketIdOrSlug,
        _change: &TicketStateChange,
    ) -> Result<(), BackendWorkspaceClientError> {
        Err(BackendWorkspaceClientError::InvalidTarget(
            UNSUPPORTED_TICKET_STATE_CHANGE.into(),
        ))
    }

    pub fn close_ticket(
        &self,
        _id: &TicketIdOrSlug,
        _resolution: &MarkdownText,
    ) -> Result<(), BackendWorkspaceClientError> {
        Err(BackendWorkspaceClientError::InvalidTarget(
            UNSUPPORTED_TICKET_STATE_CHANGE.into(),
        ))
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

    pub fn list_subjektiv_subjects(
        &self,
        limit: usize,
    ) -> Result<SubjektivSubjectListResponse, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        self.generated(move |client| async move {
            client
                .subjektiv_subject_list(
                    workspace_id,
                    SubjektivSubjectListQuery {
                        limit: Some(limit),
                        cursor: None,
                    },
                )
                .await
        })
    }

    pub fn subjektiv_subject(
        &self,
        subject_id: &str,
    ) -> Result<SubjektivSubjectResponse, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let subject_id = subject_id.to_string();
        self.generated(move |client| async move {
            client.subjektiv_subject_get(workspace_id, subject_id).await
        })
    }

    pub fn subjektiv_resident_surface(
        &self,
        subject_id: &str,
    ) -> Result<SubjektivResidentSurfaceResponse, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let subject_id = subject_id.to_string();
        self.generated(move |client| async move {
            client
                .subjektiv_resident_surface(workspace_id, subject_id)
                .await
        })
    }

    pub fn list_subjektiv_memories(
        &self,
        subject_id: &str,
        query: &SubjektivMemoryListQuery,
    ) -> Result<SubjektivMemoryQueryResponse, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let subject_id = subject_id.to_string();
        let query = query.clone();
        self.generated(move |client| async move {
            client
                .subjektiv_memory_list(workspace_id, subject_id, query)
                .await
        })
    }

    pub fn subjektiv_memory(
        &self,
        subject_id: &str,
        memory_id: &str,
        query: &SubjektivMemoryDetailQuery,
    ) -> Result<SubjektivMemoryReadResponse, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let subject_id = subject_id.to_string();
        let memory_id = memory_id.to_string();
        let query = query.clone();
        self.generated(move |client| async move {
            client
                .subjektiv_memory_detail(workspace_id, subject_id, memory_id, query)
                .await
        })
    }

    pub fn list_subjektiv_memory_changes(
        &self,
        subject_id: &str,
        memory_id: &str,
        query: &SubjektivMemoryChangesQuery,
    ) -> Result<SubjektivMemoryListChangesResponse, BackendWorkspaceClientError> {
        let workspace_id = self.workspace_id.clone();
        let subject_id = subject_id.to_string();
        let memory_id = memory_id.to_string();
        let query = query.clone();
        self.generated(move |client| async move {
            client
                .subjektiv_memory_changes(workspace_id, subject_id, memory_id, query)
                .await
        })
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
            .find(|runtime| runtime.worker_creation_available)
            .ok_or_else(|| {
                BackendWorkspaceClientError::InvalidTarget(
                    "Backend has no spawn-capable Runtime that supports a Workdir-less Intake Worker"
                        .to_string(),
                )
            })?;
        let request = CreateWorkspaceWorkerRequest {
            runtime_id: runtime.runtime_id.clone(),
            display_name: format!("intake-{ticket_id}"),
            singleton_key: None,
            profile: Some("builtin:intake".to_string()),
            ticket_assignment: None,
            initial_submit: vec![protocol::Segment::Text {
                content: format!("Please handle intake for Ticket {ticket_id}."),
            }],
            workdir_attachments: Vec::new(),
            feature_connections: Default::default(),
            control_operation_id: None,
            execution_grant: None,
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
    fn edit_post_with_snapshots(
        &self,
        id: TicketIdOrSlug,
        edit: ticket::TicketPostEdit,
    ) -> ticket::Result<(Ticket, Ticket)> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket_reference(&id);
        self.generated(move |client| async move {
            client
                .ticket_record_post_edit(workspace_id, id, server_api::EditTicketPostRequest(edit))
                .await
                .map(|response| response.0)
        })
        .map_err(ticket_client_error)
    }

    fn list_workspace_tags(
        &self,
        request: ticket::TagListRequest,
    ) -> ticket::Result<Vec<ticket::WorkspaceTag>> {
        BackendWorkspaceProductClient::list_workspace_tags(self, &request)
            .map_err(ticket_client_error)
    }

    fn mutate_workspace_tag(
        &self,
        request: ticket::WorkspaceTagMutation,
    ) -> ticket::Result<ticket::WorkspaceTag> {
        BackendWorkspaceProductClient::mutate_workspace_tag(self, &request)
            .map_err(ticket_client_error)
    }

    fn set_ticket_tags(
        &self,
        id: TicketIdOrSlug,
        request: ticket::TicketTagSetRequest,
    ) -> ticket::Result<Vec<ticket::WorkspaceTag>> {
        BackendWorkspaceProductClient::set_ticket_tags(self, &id, &request)
            .map_err(ticket_client_error)
    }

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
        _id: TicketIdOrSlug,
        _change: TicketStateChange,
    ) -> ticket::Result<()> {
        Err(TicketError::Conflict(
            UNSUPPORTED_TICKET_STATE_CHANGE.into(),
        ))
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

    fn update_case(
        &self,
        ticket: &str,
        request: ticket::TicketCaseUpdate,
    ) -> ticket::Result<Ticket> {
        let workspace_id = self.workspace_id.clone();
        let id = ticket.to_owned();
        // Actor identity belongs to authenticated Backend authority, not the caller.
        let request = server_api::TicketCaseUpdateRequest {
            expected_content_digest: request.expected_content_digest,
            expected_state: request.expected_state,
            expected_closure_operation_key: request.expected_closure_operation_key,
            operation_key: request.operation_key,
            action: request.action,
            explanation: request.explanation,
            references: request.references,
        };
        self.generated(move |client| async move {
            client
                .ticket_case_update(workspace_id.clone(), id.clone(), request)
                .await?;
            // The case endpoint returns only a summary. Fetch the authoritative
            // full record rather than fabricating body, events, or case history.
            client
                .ticket_record_get(workspace_id, id)
                .await
                .map(|response| response.0)
        })
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

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let bytes = stream.read(&mut buffer).unwrap();
            assert_ne!(bytes, 0, "incomplete HTTP request");
            request.extend_from_slice(&buffer[..bytes]);
            if let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..header_end]).unwrap();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + content_length {
                    return String::from_utf8(request).unwrap();
                }
            }
        }
    }

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
            sender.send(read_http_request(&mut stream)).unwrap();
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
        responses: Vec<(&str, &str)>,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let responses = responses
            .into_iter()
            .map(|(status, body)| (status.to_owned(), body.to_owned()))
            .collect::<Vec<_>>();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                sender.send(read_http_request(&mut stream)).unwrap();
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
    fn post_edit_uses_native_patch_and_returns_server_snapshots_without_reread() {
        let temp = tempfile::tempdir().unwrap();
        let backend =
            ticket::SqliteTicketBackend::open(temp.path().join("fixture.db"), "workspace-a")
                .unwrap();
        let mut input = NewTicket::new("Post snapshots");
        input.body = MarkdownText::new("Original requirements");
        let reference = backend.create(input).unwrap();
        let original = backend.show(reference.id.clone().into()).unwrap();
        let edit = ticket::TicketPostEdit {
            event_ref: original.events[0].event_ref.clone(),
            expected_body: original.document.body.clone(),
            body: MarkdownText::new("Changed requirements"),
            author: "authenticated-editor".into(),
        };
        let expected = backend
            .edit_post_with_snapshots(reference.id.into(), edit.clone())
            .unwrap();
        let body = serde_json::to_string(&expected).unwrap();
        let (url, requests, server) = one_response_server("200 OK", &body);
        let client =
            BackendWorkspaceProductClient::new_with_access_token(url, "workspace-a", "token")
                .unwrap();
        let actual = TicketBackend::edit_post_with_snapshots(
            &client,
            TicketIdOrSlug::Query("T-123".into()),
            edit.clone(),
        )
        .unwrap();
        assert_eq!(actual, expected);
        let request = requests.recv().unwrap();
        assert!(request.starts_with("PATCH /api/w/workspace-a/tickets/T-123/posts HTTP/1.1\r\n"));
        let (_, body) = request.split_once("\r\n\r\n").unwrap();
        assert_eq!(
            serde_json::from_str::<ticket::TicketPostEdit>(body).unwrap(),
            edit
        );
        server.join().unwrap();
    }

    #[test]
    fn legacy_state_and_close_calls_require_explicit_case_cas_without_http() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let client = BackendWorkspaceProductClient::new_with_access_token(
            format!("http://{}", listener.local_addr().unwrap()),
            "workspace-a",
            "token",
        )
        .unwrap();
        let id = TicketIdOrSlug::Query("T-739".into());
        let change = TicketStateChange::new("open", "closed", "Judgment", "Explanation");
        let errors = [
            client
                .set_ticket_workflow_state(&id, &change)
                .unwrap_err()
                .to_string(),
            client
                .close_ticket(&id, &MarkdownText::new("Withdrawn"))
                .unwrap_err()
                .to_string(),
            TicketBackend::add_state_changed(&client, id, change)
                .unwrap_err()
                .to_string(),
        ];
        for error in errors {
            assert!(
                error.contains("unsupported legacy Ticket state/close operation"),
                "{error}"
            );
            assert!(error.contains("TicketBackend::update_case"), "{error}");
            assert!(error.contains("caller-supplied"), "{error}");
            assert!(error.contains("CAS"), "{error}");
        }
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn serialized_update_case_forwards_tagged_actions_and_returns_full_backend_record() {
        for action in [
            serde_json::json!({"action": "close", "reason": "completed"}),
            serde_json::json!({"action": "close", "reason": {"other": "Custom judgment"}}),
            serde_json::json!({"action": "correct_closure", "reason": "duplicate"}),
            serde_json::json!({"action": "reopen"}),
        ] {
            // SQLite only supplies hermetic response fixtures, never a client fallback.
            let temp = tempfile::tempdir().unwrap();
            let backend =
                ticket::SqliteTicketBackend::open(temp.path().join("fixture.db"), "workspace-a")
                    .unwrap();
            let mut input = NewTicket::new("Full case record");
            input.body = MarkdownText::new("Full body absent from the endpoint summary");
            let reference = backend.create(input).unwrap();
            let tag = backend
                .mutate_workspace_tag(ticket::WorkspaceTagMutation {
                    operation_key: "classify-create".into(),
                    action: ticket::WorkspaceTagAction::Create {
                        name: "closed".into(),
                    },
                })
                .unwrap();
            backend
                .set_ticket_tags(
                    reference.id.clone().into(),
                    ticket::TicketTagSetRequest {
                        operation_key: "classify-set".into(),
                        expected_tag_ids: Vec::new(),
                        tag_ids: vec![tag.id.clone()],
                    },
                )
                .unwrap();
            let mut snapshot = backend.show(reference.id.clone().into()).unwrap();
            if action["action"] != "close" {
                snapshot = backend
                    .update_case(
                        &reference.id,
                        ticket::TicketCaseUpdate {
                            expected_content_digest: ticket::ticket_content_digest(&snapshot),
                            expected_state: ticket::TicketState::Open,
                            expected_closure_operation_key: None,
                            operation_key: "initial-close".into(),
                            action: ticket::TicketCaseAction::Close {
                                reason: ticket::TicketClosureReason::Unknown,
                            },
                            explanation: "Initial judgment".into(),
                            references: Vec::new(),
                            author: Some("authenticated-actor".into()),
                        },
                    )
                    .unwrap();
            }
            let expected_request = serde_json::json!({
                "expected_content_digest": ticket::ticket_content_digest(&snapshot),
                "expected_state": snapshot.meta.workflow_state.as_str(),
                "expected_closure_operation_key": snapshot.closure_operation_key,
                "operation_key": "caller-case-key",
                "action": action,
                "explanation": "Caller judgment, not work progress",
                "references": [{"kind": "ticket", "target": "T-738"}],
            });
            let mut domain_request = expected_request.clone();
            domain_request["author"] = serde_json::json!("caller-cannot-impersonate");
            let operation =
                serde_json::from_value::<ticket::TicketBackendOperation>(serde_json::json!({
                    "operation": "update_case",
                    "ticket": reference.id,
                    "request": domain_request,
                }))
                .unwrap();
            let mut fixture_request: ticket::TicketCaseUpdate =
                serde_json::from_value(expected_request.clone()).unwrap();
            fixture_request.author = Some("authenticated-actor".into());
            let expected = backend.update_case(&reference.id, fixture_request).unwrap();
            let summary =
                serde_json::to_string(&backend.list(TicketListQuery::all()).unwrap()[0]).unwrap();
            let full_record = serde_json::to_string(&expected).unwrap();
            let (base_url, requests, handle) =
                response_sequence_server(vec![("200 OK", &summary), ("200 OK", &full_record)]);
            let client = BackendWorkspaceProductClient::new_with_access_token(
                base_url,
                "workspace-a",
                "test-token",
            )
            .unwrap();
            let result = ticket::execute_ticket_backend_operation(&client, operation).unwrap();
            let ticket::TicketBackendOperationResult::Ticket(actual) = result else {
                panic!("case operation must return a full Ticket");
            };
            assert_eq!(actual, expected);
            assert_eq!(actual.tags, vec![tag]);
            assert!(!actual.case_history.is_empty());
            assert_eq!(actual.closure_operation_key, expected.closure_operation_key);
            let post = requests.recv().unwrap();
            assert!(
                post.starts_with(&format!(
                    "POST /workspaces/workspace-a/tickets/{}/case ",
                    reference.id,
                )),
                "{post}"
            );
            assert!(
                post.contains("authorization: Bearer test-token\r\n"),
                "{post}"
            );
            let body: serde_json::Value =
                serde_json::from_str(post.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(body, expected_request);
            assert!(body.get("author").is_none());
            assert!(requests.recv().unwrap().starts_with(&format!(
                "GET /api/w/workspace-a/tickets/{}/record ",
                reference.id,
            )));
            handle.join().unwrap();
        }
    }

    #[test]
    fn classification_client_uses_scoped_routes_and_preserves_cas_and_replay_input() {
        let tag = serde_json::json!({"workspace_id":"workspace-a", "id":"tag-a", "name":"Ready", "last_operation_key":"create-1", "deleted":false});
        let single = tag.to_string();
        let list = serde_json::json!([tag]).to_string();
        let (base_url, requests, handle) = response_sequence_server(vec![
            ("200 OK", &list),
            ("200 OK", &single),
            ("200 OK", &list),
        ]);
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-token",
        )
        .unwrap();
        let listed = client
            .list_workspace_tags(&ticket::TagListRequest {
                after: Some("tag-0".into()),
                limit: Some(7),
            })
            .unwrap();
        assert_eq!(listed[0].id, "tag-a");
        let request = requests.recv().unwrap();
        assert!(
            request.starts_with("GET /api/w/workspace-a/ticket-tags?"),
            "{request}"
        );
        assert!(
            request.contains("after=tag-0") && request.contains("limit=7"),
            "{request}"
        );
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer test-token")
        );
        let mutation = ticket::WorkspaceTagMutation {
            operation_key: "rename-1".into(),
            action: ticket::WorkspaceTagAction::Rename {
                tag_id: "tag-a".into(),
                expected_last_operation_key: "create-1".into(),
                name: "New".into(),
            },
        };
        TicketBackend::mutate_workspace_tag(&client, mutation.clone()).unwrap();
        let request = requests.recv().unwrap();
        assert!(
            request.starts_with("POST /api/w/workspace-a/ticket-tags "),
            "{request}"
        );
        let body: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, serde_json::json!(mutation));
        let set = ticket::TicketTagSetRequest {
            operation_key: "set-1".into(),
            expected_tag_ids: vec!["tag-old".into()],
            tag_ids: vec!["tag-a".into(), "tag-b".into()],
        };
        let receipt =
            TicketBackend::set_ticket_tags(&client, TicketIdOrSlug::from("T-740"), set.clone())
                .unwrap();
        assert_eq!(receipt[0].name, "Ready");
        let request = requests.recv().unwrap();
        assert!(
            request.starts_with("PUT /api/w/workspace-a/tickets/T-740/tags "),
            "{request}"
        );
        let body: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, serde_json::json!(set));
        handle.join().unwrap();
    }

    #[test]
    fn classification_client_returns_backend_denials_without_local_fallback() {
        for status in ["403 Forbidden", "409 Conflict"] {
            let (base_url, requests, handle) = one_response_server(
                status,
                r#"{"error":"ticket_conflict","message":"stale or unauthorized","diagnostics":[]}"#,
            );
            let client = BackendWorkspaceProductClient::new_with_access_token(
                base_url,
                "workspace-a",
                "token",
            )
            .unwrap();
            let result = client.set_ticket_tags(
                &TicketIdOrSlug::from("T-740"),
                &ticket::TicketTagSetRequest {
                    operation_key: "retry-key".into(),
                    expected_tag_ids: vec![],
                    tag_ids: vec!["tag-a".into()],
                },
            );
            assert!(result.is_err());
            assert!(
                requests
                    .recv()
                    .unwrap()
                    .starts_with("PUT /api/w/workspace-a/tickets/T-740/tags ")
            );
            handle.join().unwrap();
        }
    }

    fn case_request() -> ticket::TicketCaseUpdate {
        ticket::TicketCaseUpdate {
            operation_key: "caller-case-key".into(),
            expected_content_digest: "caller-content-digest".into(),
            expected_state: ticket::TicketState::Closed,
            expected_closure_operation_key: Some("last-close-key".into()),
            action: ticket::TicketCaseAction::Reopen,
            explanation: "Caller reopening judgment".into(),
            references: Vec::new(),
            author: None,
        }
    }

    #[test]
    fn case_update_rejection_preserves_caller_cas_without_local_fallback() {
        for (status, body, expected_error) in [
            (
                "409 Conflict",
                r#"{"error":"ticket_conflict","message":"stale content","diagnostics":[]}"#,
                "409",
            ),
            (
                "403 Forbidden",
                r#"{"error":"forbidden","message":"not authorized","diagnostics":[]}"#,
                "403",
            ),
        ] {
            let (base_url, requests, handle) = one_response_server(status, body);
            let client = BackendWorkspaceProductClient::new_with_access_token(
                base_url,
                "workspace-a",
                "test-token",
            )
            .unwrap();
            let error = TicketBackend::update_case(&client, "T-739", case_request()).unwrap_err();
            assert!(
                error.to_string().contains(expected_error),
                "{error}; response {status}: {body}"
            );
            let request = requests.recv().unwrap();
            assert!(
                request.starts_with("POST /workspaces/workspace-a/tickets/T-739/case "),
                "{request}"
            );
            let body: serde_json::Value =
                serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(body["expected_content_digest"], "caller-content-digest");
            assert_eq!(body["expected_state"], "closed");
            assert_eq!(body["expected_closure_operation_key"], "last-close-key");
            assert_eq!(body["operation_key"], "caller-case-key");
            assert_eq!(body["action"], serde_json::json!({"action": "reopen"}));
            assert!(!error.to_string().contains("test-token"));
            handle.join().unwrap();
            assert!(requests.try_recv().is_err());
        }
    }

    #[test]
    fn successful_case_update_with_failed_show_does_not_fabricate_full_ticket() {
        let temp = tempfile::tempdir().unwrap();
        let backend =
            ticket::SqliteTicketBackend::open(temp.path().join("fixture.db"), "workspace-a")
                .unwrap();
        let reference = backend.create(NewTicket::new("Summary only")).unwrap();
        let summary =
            serde_json::to_string(&backend.list(TicketListQuery::all()).unwrap()[0]).unwrap();
        let (base_url, requests, handle) = response_sequence_server(vec![
            ("200 OK", &summary),
            (
                "404 Not Found",
                r#"{"error":"ticket_not_found","message":"full record unavailable","diagnostics":[]}"#,
            ),
        ]);
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-token",
        )
        .unwrap();
        let error = TicketBackend::update_case(&client, &reference.id, case_request()).unwrap_err();
        assert!(error.to_string().contains("404"), "{error}");
        assert!(requests.recv().unwrap().starts_with("POST /workspaces/"));
        assert!(requests.recv().unwrap().starts_with("GET /api/w/"));
        handle.join().unwrap();
        assert!(requests.try_recv().is_err());
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
    fn subjektiv_memory_list_encodes_typed_filters_as_bounded_query_scalars() {
        let (base_url, request, handle) =
            one_response_server("200 OK", r#"{"items":[],"has_more":false}"#);
        let client = BackendWorkspaceProductClient::new_with_access_token(
            base_url,
            "workspace-a",
            "test-backend-token",
        )
        .unwrap();

        let response = client
            .list_subjektiv_memories(
                "subject/a",
                &SubjektivMemoryListQuery {
                    query: Some("stable cursor".to_string()),
                    kinds: None,
                    states: Some(vec![
                        server_api::SubjektivMemoryState::Active,
                        server_api::SubjektivMemoryState::Resolved,
                    ]),
                    limit: Some(25),
                    cursor: None,
                },
            )
            .unwrap();

        assert!(response.items.is_empty());
        let request = request.recv().unwrap();
        assert!(
            request.starts_with("GET /api/w/workspace-a/subjektiv/subjects/subject%2Fa/memories?")
        );
        assert!(request.contains("query=stable+cursor"));
        assert!(request.contains("states=active%2Cresolved"));
        assert!(request.contains("limit=25"));
        assert!(request.contains("authorization: Bearer test-backend-token\r\n"));
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
                r#"{"workspace_id":"workspace-a","runtimes":[{"runtime_id":"embedded","display_name":"Embedded","built_in":true,"worker_creation_available":true,"supports_workdir_attachments":false,"status":"connected","diagnostics":[]}],"default_profile":null,"profiles":[],"repositories":[],"working_directories":[],"diagnostics":[]}"#,
            ),
            (
                "200 OK",
                r#"{"workspace_id":"workspace-a","runtime_id":"embedded","worker_id":"worker-1","console_href":"/w/workspace-a/workers/worker-1","worker":{"runtime_id":"embedded","worker_id":"worker-1","host_id":"embedded","display_name":"Intake","label":"worker-1","profile":"builtin:intake","singleton_key":null,"tags":[],"workspace":{"visibility":"workspace","identity":"workspace-a","workspace_id":"workspace-a"},"state":"idle","last_seen_at":null,"pinned":false,"retention_state":"active","implementation":{"kind":"runtime","display_hint":"Runtime Worker"},"diagnostics":[]},"diagnostics":[]}"#,
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
        let body = r#"{"workspace_id":"workspace-a","online":true,"disposition":"created","worker":{"runtime_id":"embedded","worker_id":"worker-2","host_id":"embedded","display_name":"Orchestrator","label":"worker-2","profile":"builtin:orchestrator","singleton_key":"workspace-orchestrator","tags":[],"workspace":{"visibility":"workspace","identity":"workspace-a","workspace_id":"workspace-a"},"state":"idle","last_seen_at":null,"pinned":true,"retention_state":"active","implementation":{"kind":"runtime","display_hint":"Runtime Worker"},"diagnostics":[]},"diagnostics":[]}"#;
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
                    [TicketListState::Open, TicketListState::Closed]
                        .into_iter()
                        .collect(),
                ),
            }),
            "open,closed"
        );
    }

    #[test]
    fn ticket_and_objective_references_are_path_encoded() {
        assert_eq!(encode_path_segment("T-1/a"), "T-1%2Fa");
    }
}
