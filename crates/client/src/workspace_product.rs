use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use ticket::{
    MarkdownText, NewOrchestrationPlanRecord, NewTicket, NewTicketEvent, NewTicketRelation,
    OrchestrationPlanKind, OrchestrationPlanRecord, Ticket, TicketBackend, TicketDependencyCheck,
    TicketDoctorReport, TicketError, TicketIdOrSlug, TicketIntakeSummary, TicketItemEdit,
    TicketListQuery, TicketListState, TicketMarkReady, TicketRef, TicketRelation,
    TicketRelationKind, TicketRelationView, TicketStateChange, TicketStateSelector, TicketSummary,
};
use workspace_api::{
    ListResponse, ObjectiveCreateRequest, ObjectiveDetail, ObjectiveEditRequest,
    ObjectiveLinkTicketRequest, ObjectiveStateRequest, ObjectiveSummary,
};

use crate::BackendWorkspaceClientError;

const DEFAULT_PRODUCT_LIST_LIMIT: usize = 1_000;

#[derive(Debug, Deserialize)]
struct BackendWorkerLaunchOptions {
    runtimes: Vec<BackendWorkerLaunchRuntime>,
}

#[derive(Debug, Deserialize)]
struct BackendWorkerLaunchRuntime {
    runtime_id: String,
    can_spawn_worker: bool,
    working_directory_required: bool,
}

#[derive(Debug, Deserialize)]
struct BackendCreateWorkerResponse {
    runtime_id: String,
    worker_id: String,
}

#[derive(Debug, Deserialize)]
struct BackendWorkspaceOrchestratorResponse {
    disposition: String,
    worker: Option<BackendCreateWorkerResponse>,
}

/// Workspace-scoped Backend client for Ticket and Objective product state.
///
/// Construction requires both the selected Backend URL and Workspace identity.
/// Callers should derive these once from `Target::resolve()` and must not retry
/// failed requests against repository-local state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendWorkspaceProductClient {
    base_url: String,
    workspace_id: String,
}

impl BackendWorkspaceProductClient {
    pub fn new(
        base_url: impl Into<String>,
        workspace_id: impl Into<String>,
    ) -> Result<Self, BackendWorkspaceClientError> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            return Err(BackendWorkspaceClientError::InvalidTarget(
                "Backend base URL must not be empty".into(),
            ));
        }
        let workspace_id = workspace_id.into();
        if workspace_id.trim().is_empty() {
            return Err(BackendWorkspaceClientError::InvalidTarget(
                "Backend Workspace identity must not be empty".into(),
            ));
        }
        Ok(Self {
            base_url,
            workspace_id,
        })
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn list_tickets(
        &self,
        query: &TicketListQuery,
    ) -> Result<Vec<TicketSummary>, BackendWorkspaceClientError> {
        let state = ticket_list_state_query(query);
        self.get_json(&format!("/tickets/search?state={state}"))
    }

    pub fn show_ticket(&self, id: &TicketIdOrSlug) -> Result<Ticket, BackendWorkspaceClientError> {
        self.get_json(&format!(
            "/tickets/{}/record",
            encode_path_segment(&ticket_reference(id))
        ))
    }

    pub fn create_ticket(
        &self,
        input: &NewTicket,
    ) -> Result<TicketRef, BackendWorkspaceClientError> {
        self.send_json(Method::POST, "/tickets", Some(input))
    }

    pub fn add_ticket_event(
        &self,
        id: &TicketIdOrSlug,
        event: &NewTicketEvent,
    ) -> Result<(), BackendWorkspaceClientError> {
        self.send_unit(
            Method::POST,
            &format!(
                "/tickets/{}/thread-events",
                encode_path_segment(&ticket_reference(id))
            ),
            Some(event),
        )
    }

    pub fn set_ticket_workflow_state(
        &self,
        id: &TicketIdOrSlug,
        change: &TicketStateChange,
    ) -> Result<(), BackendWorkspaceClientError> {
        self.send_unit(
            Method::POST,
            &format!(
                "/tickets/{}/workflow-state",
                encode_path_segment(&ticket_reference(id))
            ),
            Some(change),
        )
    }

    pub fn close_ticket(
        &self,
        id: &TicketIdOrSlug,
        resolution: &MarkdownText,
    ) -> Result<(), BackendWorkspaceClientError> {
        self.send_unit(
            Method::POST,
            &format!(
                "/tickets/{}/workflow/close",
                encode_path_segment(&ticket_reference(id))
            ),
            Some(resolution),
        )
    }

    pub fn add_ticket_relation(
        &self,
        id: &TicketIdOrSlug,
        relation: &NewTicketRelation,
    ) -> Result<TicketRelation, BackendWorkspaceClientError> {
        self.send_json(
            Method::POST,
            &format!(
                "/tickets/{}/relations",
                encode_path_segment(&ticket_reference(id))
            ),
            Some(relation),
        )
    }

    pub fn query_ticket_relations(
        &self,
        ticket: Option<&TicketIdOrSlug>,
        kind: Option<TicketRelationKind>,
    ) -> Result<Vec<TicketRelation>, BackendWorkspaceClientError> {
        #[derive(Serialize)]
        struct Query<'a> {
            ticket: Option<&'a TicketIdOrSlug>,
            kind: Option<TicketRelationKind>,
        }
        self.send_json(
            Method::POST,
            "/tickets/relations/search",
            Some(&Query { ticket, kind }),
        )
    }

    pub fn ticket_doctor(&self) -> Result<TicketDoctorReport, BackendWorkspaceClientError> {
        self.get_json("/tickets/doctor")
    }

    pub fn list_objectives(
        &self,
        limit: usize,
    ) -> Result<ListResponse<ObjectiveSummary>, BackendWorkspaceClientError> {
        self.get_json(&format!("/objectives?limit={limit}"))
    }

    pub fn show_objective(&self, id: &str) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        self.get_json(&format!("/objectives/{}", encode_path_segment(id)))
    }

    pub fn create_objective(
        &self,
        input: &ObjectiveCreateRequest,
    ) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        self.send_json(Method::POST, "/objectives", Some(input))
    }

    pub fn edit_objective(
        &self,
        id: &str,
        input: &ObjectiveEditRequest,
    ) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        self.send_json(
            Method::PATCH,
            &format!("/objectives/{}", encode_path_segment(id)),
            Some(input),
        )
    }

    pub fn set_objective_state(
        &self,
        id: &str,
        input: &ObjectiveStateRequest,
    ) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        self.send_json(
            Method::POST,
            &format!("/objectives/{}/state", encode_path_segment(id)),
            Some(input),
        )
    }

    pub fn link_objective_ticket(
        &self,
        id: &str,
        input: &ObjectiveLinkTicketRequest,
    ) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        self.send_json(
            Method::POST,
            &format!("/objectives/{}/ticket-links", encode_path_segment(id)),
            Some(input),
        )
    }

    pub fn unlink_objective_ticket(
        &self,
        id: &str,
        ticket_id: &str,
    ) -> Result<ObjectiveDetail, BackendWorkspaceClientError> {
        self.send_json::<(), _>(
            Method::DELETE,
            &format!(
                "/objectives/{}/ticket-links/{}",
                encode_path_segment(id),
                encode_path_segment(ticket_id)
            ),
            None,
        )
    }

    pub fn launch_ticket_intake(
        &self,
        ticket_id: &str,
    ) -> Result<String, BackendWorkspaceClientError> {
        let options: BackendWorkerLaunchOptions = self.get_json("/workers/launch-options")?;
        let runtime = options
            .runtimes
            .iter()
            .find(|runtime| runtime.can_spawn_worker && !runtime.working_directory_required)
            .ok_or_else(|| {
                BackendWorkspaceClientError::InvalidTarget(
                    "Backend has no spawn-capable Runtime that supports a Workdir-less Intake Worker"
                        .to_string(),
                )
            })?;
        let response: BackendCreateWorkerResponse = self.send_json(
            Method::POST,
            "/workers",
            Some(&serde_json::json!({
                "runtime_id": runtime.runtime_id,
                "display_name": format!("intake-{ticket_id}"),
                "profile": "builtin:intake",
                "initial_submit": [{
                    "kind": "text",
                    "content": format!("Please handle intake for Ticket {ticket_id}.")
                }]
            })),
        )?;
        Ok(format!(
            "Started Intake Worker {}/{} for Ticket {ticket_id}",
            response.runtime_id, response.worker_id
        ))
    }

    pub fn start_workspace_orchestrator(&self) -> Result<String, BackendWorkspaceClientError> {
        let response: BackendWorkspaceOrchestratorResponse =
            self.send_json::<(), _>(Method::POST, "/orchestrator", None)?;
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

    fn get_json<R: DeserializeOwned>(&self, path: &str) -> Result<R, BackendWorkspaceClientError> {
        self.send_json::<(), R>(Method::GET, path, None)
    }

    fn send_json<B: Serialize + ?Sized, R: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<R, BackendWorkspaceClientError> {
        let response = self.request(method, path, body)?.send()?;
        let response = ensure_success(response)?;
        response.json().map_err(BackendWorkspaceClientError::Http)
    }

    fn send_unit<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<(), BackendWorkspaceClientError> {
        ensure_success(self.request(method, path, body)?.send()?)?;
        Ok(())
    }

    fn request<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<reqwest::blocking::RequestBuilder, BackendWorkspaceClientError> {
        let client = reqwest::blocking::Client::builder().build()?;
        let url = format!(
            "{}/api/w/{}/{}",
            self.base_url,
            encode_path_segment(&self.workspace_id),
            path.trim_start_matches('/')
        );
        let request = client.request(method, url);
        Ok(match body {
            Some(body) => request.json(body),
            None => request,
        })
    }
}

impl TicketBackend for BackendWorkspaceProductClient {
    fn default_intake_ready_state_change_body(&self, from: &str) -> String {
        #[derive(Serialize)]
        struct Request<'a> {
            from: &'a str,
        }
        self.send_json(
            Method::POST,
            "/tickets/default-intake-ready-body",
            Some(&Request { from }),
        )
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
        self.send_json(
            Method::PATCH,
            &format!(
                "/tickets/{}/item",
                encode_path_segment(&ticket_reference(&id))
            ),
            Some(&edit),
        )
        .map_err(ticket_client_error)
    }

    fn dependency_check(&self, id: TicketIdOrSlug) -> ticket::Result<TicketDependencyCheck> {
        self.get_json(&format!(
            "/tickets/{}/dependency-check",
            encode_path_segment(&ticket_reference(&id))
        ))
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
        self.send_unit(
            Method::POST,
            &format!(
                "/tickets/{}/state-changes",
                encode_path_segment(&ticket_reference(&id))
            ),
            Some(&change),
        )
        .map_err(ticket_client_error)
    }

    fn add_intake_summary(
        &self,
        id: TicketIdOrSlug,
        summary: TicketIntakeSummary,
    ) -> ticket::Result<()> {
        self.send_unit(
            Method::POST,
            &format!(
                "/tickets/{}/intake-summaries",
                encode_path_segment(&ticket_reference(&id))
            ),
            Some(&summary),
        )
        .map_err(ticket_client_error)
    }

    fn set_state_field(
        &self,
        id: TicketIdOrSlug,
        field: &str,
        change: TicketStateChange,
    ) -> ticket::Result<()> {
        self.send_unit(
            Method::POST,
            &format!(
                "/tickets/{}/state-fields/{}",
                encode_path_segment(&ticket_reference(&id)),
                encode_path_segment(field)
            ),
            Some(&change),
        )
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
        self.send_json(
            Method::POST,
            &format!(
                "/tickets/{}/workflow/mark-ready",
                encode_path_segment(&ticket_reference(&id))
            ),
            Some(&request),
        )
        .map_err(ticket_client_error)
    }

    fn queue_ready(&self, id: TicketIdOrSlug, _queued_by: &str) -> ticket::Result<()> {
        self.send_unit::<()>(
            Method::POST,
            &format!(
                "/tickets/{}/workflow/queue",
                encode_path_segment(&ticket_reference(&id))
            ),
            None,
        )
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
        #[derive(Serialize)]
        struct Request {
            kind: TicketRelationKind,
            target: String,
        }
        self.send_json(
            Method::DELETE,
            &format!(
                "/tickets/{}/relations",
                encode_path_segment(&ticket_reference(&id))
            ),
            Some(&Request {
                kind,
                target: ticket_reference(&target),
            }),
        )
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
        self.get_json(&format!(
            "/tickets/{}/relation-view",
            encode_path_segment(&ticket_reference(&id))
        ))
        .map_err(ticket_client_error)
    }

    fn add_orchestration_plan_record(
        &self,
        id: TicketIdOrSlug,
        record: NewOrchestrationPlanRecord,
    ) -> ticket::Result<OrchestrationPlanRecord> {
        self.send_json(
            Method::POST,
            &format!(
                "/tickets/{}/orchestration-plans",
                encode_path_segment(&ticket_reference(&id))
            ),
            Some(&record),
        )
        .map_err(ticket_client_error)
    }

    fn query_orchestration_plan_records(
        &self,
        ticket: Option<TicketIdOrSlug>,
        kind: Option<OrchestrationPlanKind>,
    ) -> ticket::Result<Vec<OrchestrationPlanRecord>> {
        #[derive(Serialize)]
        struct Query {
            ticket: Option<TicketIdOrSlug>,
            kind: Option<OrchestrationPlanKind>,
        }
        self.send_json(
            Method::POST,
            "/tickets/orchestration-plans/search",
            Some(&Query { ticket, kind }),
        )
        .map_err(ticket_client_error)
    }

    fn doctor(&self) -> ticket::Result<TicketDoctorReport> {
        self.ticket_doctor().map_err(ticket_client_error)
    }
}

fn ticket_client_error(error: BackendWorkspaceClientError) -> TicketError {
    TicketError::Sqlite(format!("Backend request failed: {error}"))
}

fn ensure_success(
    response: reqwest::blocking::Response,
) -> Result<reqwest::blocking::Response, BackendWorkspaceClientError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let message = response
        .text()
        .unwrap_or_else(|_| "Backend request failed".to_string());
    Err(BackendWorkspaceClientError::RequestFailed { status, message })
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
    fn objective_list_uses_workspace_scoped_backend_route() {
        let body = r#"{"workspace_id":"workspace-a","limit":1000,"items":[],"source":"sqlite","diagnostics":[]}"#;
        let (base_url, request, handle) = one_response_server("200 OK", body);
        let client = BackendWorkspaceProductClient::new(base_url, "workspace-a").unwrap();

        let response = client.list_objectives(1_000).unwrap();

        assert!(response.items.is_empty());
        assert!(
            request
                .recv()
                .unwrap()
                .starts_with("GET /api/w/workspace-a/objectives?limit=1000 ")
        );
        handle.join().unwrap();
    }

    #[test]
    fn backend_mutation_failure_is_returned_without_local_fallback() {
        let (base_url, request, handle) = one_response_server("403 Forbidden", "denied");
        let client = BackendWorkspaceProductClient::new(base_url, "workspace-a").unwrap();

        let error = client
            .create_objective(&ObjectiveCreateRequest {
                title: "Objective".to_string(),
                body_md: "body".to_string(),
                state: "active".to_string(),
                linked_tickets: Vec::new(),
            })
            .unwrap_err();

        assert!(error.to_string().contains("403"));
        assert!(
            request
                .recv()
                .unwrap()
                .starts_with("POST /api/w/workspace-a/objectives ")
        );
        handle.join().unwrap();
    }

    #[test]
    fn ticket_intake_launch_uses_backend_options_and_workspace_worker_route() {
        let (base_url, requests, handle) = response_sequence_server(vec![
            (
                "200 OK",
                r#"{"runtimes":[{"runtime_id":"embedded","can_spawn_worker":true,"working_directory_required":false}]}"#,
            ),
            (
                "200 OK",
                r#"{"runtime_id":"embedded","worker_id":"worker-1"}"#,
            ),
        ]);
        let client = BackendWorkspaceProductClient::new(base_url, "workspace-a").unwrap();

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
        let body = r#"{"disposition":"created","worker":{"runtime_id":"embedded","worker_id":"worker-2"}}"#;
        let (base_url, request, handle) = one_response_server("200 OK", body);
        let client = BackendWorkspaceProductClient::new(base_url, "workspace-a").unwrap();

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
        let error = BackendWorkspaceProductClient::new("http://127.0.0.1:8787", "").unwrap_err();
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
