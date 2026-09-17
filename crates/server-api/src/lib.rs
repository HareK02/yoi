//! Generated Workspace Server REST contract for public clients.

use api_macros::api;
pub use api_macros::axum as server_support;
pub use api_macros::reqwest as client_support;
pub use api_macros::{ApiContract, HttpMethod};
use serde::{Deserialize, Serialize};
use workspace_api::WorkspaceWorkerSubject;

pub type ServerApiError = runtime_api::RuntimeApiError;
pub type ServerApiClientError = client_support::ClientError<ServerApiError>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceWorkerSessionResponse {
    pub subject: WorkspaceWorkerSubject,
    #[serde(flatten)]
    pub observation: runtime_api::WorkerSessionAvailability,
}

#[api(reqwest, axum)]
pub trait ServerApi {
    #[get(
        "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/session",
        status = 200,
        error_status = 400
    )]
    async fn worker_session(
        &self,
        #[path] workspace_id: String,
        #[path] runtime_id: String,
        #[path] worker_id: String,
    ) -> Result<WorkspaceWorkerSessionResponse, ServerApiError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_session_contract_and_flattened_availability_are_stable() {
        let operations = ServerApiMetadata::OPERATIONS;
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].method, HttpMethod::Get);
        assert_eq!(
            operations[0].path,
            "/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/session"
        );

        let response = WorkspaceWorkerSessionResponse {
            subject: WorkspaceWorkerSubject::RuntimeWorker {
                runtime_id: "runtime-a".to_string(),
                worker_id: "worker-a".to_string(),
            },
            observation: runtime_api::WorkerSessionAvailability::LiveProtocol,
        };
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["availability"], "live_protocol");
        assert_eq!(value["subject"]["kind"], "runtime_worker");
    }
}
