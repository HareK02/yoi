//! Fail-closed Drive boundary for parent-owned Internal SubWorkers, including Reviewers.
//!
//! Internal children have no Backend Drive identity. Explicit internal Drive delegation is not
//! supported until the Backend can approve scope bound to that child identity. A parent's current
//! grant, enabled profile, or Reviewer attestation is not a child Drive grant. Registered Runtime
//! Workers must use their own identity-bound, current Backend grants; this wrapper creates neither
//! identities nor grants and keeps no local authorization cache.

use std::{sync::Arc, time::Duration};

use crate::worker::{
    ReviewerContext, WorkerWorkspaceContext, WorkspaceBinaryRequest, WorkspaceBinaryResponse,
    WorkspaceClient, WorkspaceClientError, WorkspacePromptCatalogResolution, WorkspaceRequest,
    WorkspaceResponse, WorkspaceServerOperation, WorkspaceWorkerDiscoveryRequest,
};

/// Removes Drive authority without changing the selected client's other authority or behavior.
/// Install outside any Reviewer client so its trusted review submission attestation is preserved.
#[derive(Debug)]
pub(crate) struct InternalChildWorkspaceClient {
    inner: Arc<dyn WorkspaceClient>,
}

impl InternalChildWorkspaceClient {
    pub(crate) fn new(inner: Arc<dyn WorkspaceClient>) -> Self {
        Self { inner }
    }

    fn check_path(path: &str) -> Result<(), WorkspaceClientError> {
        // Classify the route, not the method: even readonly Drive access must not inherit the
        // parent's identity. Decode route segments and normalize dot segments before checking,
        // so transport/router normalization cannot turn an allowed alias into a Drive request.
        let path = path.split(['?', '#']).next().unwrap_or(path);
        let mut decoded = Vec::with_capacity(path.len());
        // HTTP URL parsers discard literal tabs/newlines before route normalization.
        let bytes: Vec<_> = path
            .bytes()
            .filter(|byte| !matches!(byte, b'\t' | b'\r' | b'\n'))
            .collect();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%'
                && index + 2 < bytes.len()
                && let (Some(high), Some(low)) =
                    (hex_digit(bytes[index + 1]), hex_digit(bytes[index + 2]))
            {
                decoded.push(high * 16 + low);
                index += 3;
            } else {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
        let decoded = String::from_utf8_lossy(&decoded).replace('\\', "/");
        let mut segments = Vec::new();
        for segment in decoded.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    segments.pop();
                }
                segment => segments.push(segment),
            }
        }
        if matches!(segments.as_slice(), ["api", "w", _, "drive", ..]) {
            return Err(WorkspaceClientError::Unavailable(
                "Internal SubWorker Drive access denied: no identity-bound Backend-approved child scope; explicit internal Drive delegation is not supported".into(),
            ));
        }
        Ok(())
    }
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Keep Workspace identity and prompt context, but never inherit Drive authority.
pub(crate) fn restrict_internal_child_drive(
    context: WorkerWorkspaceContext,
) -> WorkerWorkspaceContext {
    WorkerWorkspaceContext::with_client(
        context.workspace_id().cloned(),
        Arc::new(InternalChildWorkspaceClient::new(context.client_handle())),
    )
}

impl WorkspaceClient for InternalChildWorkspaceClient {
    fn workspace_id(&self) -> Option<&str> {
        self.inner.workspace_id()
    }
    fn kind(&self) -> &str {
        self.inner.kind()
    }
    fn is_available(&self) -> bool {
        self.inner.is_available()
    }

    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        Self::check_path(&request.path)?;
        self.inner.execute(request)
    }

    fn execute_with_timeout(
        &self,
        request: WorkspaceRequest,
        timeout: Duration,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        Self::check_path(&request.path)?;
        self.inner.execute_with_timeout(request, timeout)
    }

    fn execute_binary(
        &self,
        request: WorkspaceBinaryRequest,
    ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
        Self::check_path(&request.path)?;
        self.inner.execute_binary(request)
    }

    fn execute_binary_with_timeout(
        &self,
        request: WorkspaceBinaryRequest,
        timeout: Duration,
    ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
        Self::check_path(&request.path)?;
        self.inner.execute_binary_with_timeout(request, timeout)
    }

    fn execute_server_operation(
        &self,
        operation: WorkspaceServerOperation,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.inner.execute_server_operation(operation)
    }

    fn current_prompt_projection(
        &self,
    ) -> Result<Option<WorkspacePromptCatalogResolution>, WorkspaceClientError> {
        self.inner.current_prompt_projection()
    }

    fn list_workspace_workers(
        &self,
        request: WorkspaceWorkerDiscoveryRequest,
    ) -> Result<server_api::WorkspaceWorkerDiscoveryPage, WorkspaceClientError> {
        self.inner.list_workspace_workers(request)
    }

    fn execute_worker_remove(
        &self,
        target_runtime_id: &str,
        target_worker_id: &str,
        reason: &str,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.inner
            .execute_worker_remove(target_runtime_id, target_worker_id, reason)
    }

    fn reviewer_context(&self) -> Option<&ReviewerContext> {
        self.inner.reviewer_context()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::{ReviewerChildWorkspaceClient, WorkspaceRequestMethod};
    use std::sync::Mutex;

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Json(WorkspaceRequest, Option<Duration>),
        Binary(WorkspaceBinaryRequest, Option<Duration>),
        Projection,
        Discovery(WorkspaceWorkerDiscoveryRequest),
        Remove(String, String, String),
        Server,
    }

    #[derive(Debug, Default)]
    struct RecordingClient {
        calls: Mutex<Vec<Call>>,
    }

    impl RecordingClient {
        fn response(&self, call: Call) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.calls.lock().unwrap().push(call);
            Ok(WorkspaceResponse {
                status: 202,
                body: "forwarded".into(),
            })
        }
        fn binary_response(
            &self,
            call: Call,
        ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
            self.calls.lock().unwrap().push(call);
            Ok(WorkspaceBinaryResponse {
                status: 206,
                body: vec![0, 255],
            })
        }
    }

    impl WorkspaceClient for RecordingClient {
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
            self.response(Call::Json(request, None))
        }
        fn execute_with_timeout(
            &self,
            request: WorkspaceRequest,
            timeout: Duration,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.response(Call::Json(request, Some(timeout)))
        }
        fn execute_binary(
            &self,
            request: WorkspaceBinaryRequest,
        ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
            self.binary_response(Call::Binary(request, None))
        }
        fn execute_binary_with_timeout(
            &self,
            request: WorkspaceBinaryRequest,
            timeout: Duration,
        ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
            self.binary_response(Call::Binary(request, Some(timeout)))
        }
        fn current_prompt_projection(
            &self,
        ) -> Result<Option<WorkspacePromptCatalogResolution>, WorkspaceClientError> {
            self.calls.lock().unwrap().push(Call::Projection);
            Err(WorkspaceClientError::Request("projection marker".into()))
        }
        fn list_workspace_workers(
            &self,
            request: WorkspaceWorkerDiscoveryRequest,
        ) -> Result<server_api::WorkspaceWorkerDiscoveryPage, WorkspaceClientError> {
            self.calls.lock().unwrap().push(Call::Discovery(request));
            Err(WorkspaceClientError::Request("discovery marker".into()))
        }
        fn execute_worker_remove(
            &self,
            runtime: &str,
            worker: &str,
            reason: &str,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.response(Call::Remove(runtime.into(), worker.into(), reason.into()))
        }
        fn execute_server_operation(
            &self,
            _: WorkspaceServerOperation,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.response(Call::Server)
        }
    }

    fn binary(method: WorkspaceRequestMethod, path: &str) -> WorkspaceBinaryRequest {
        WorkspaceBinaryRequest {
            method,
            path: path.into(),
            body: Some(vec![0, 128, 255]),
            max_response_bytes: 17,
        }
    }

    #[test]
    fn internal_children_deny_all_drive_transports_without_parent_side_effects() {
        let inner = Arc::new(RecordingClient::default());
        for reviewer in [false, true] {
            let selected: Arc<dyn WorkspaceClient> = if reviewer {
                Arc::new(ReviewerChildWorkspaceClient::new(
                    inner.clone(),
                    ReviewerContext {
                        ticket_id: "T1".into(),
                        merge_request_id: "MR1".into(),
                    },
                    "attestation".into(),
                ))
            } else {
                inner.clone()
            };
            let client = InternalChildWorkspaceClient::new(selected);
            for path in [
                "/api/w/ws/drive",
                "/api/w/ws/drive?limit=1",
                "/api/w/other/drive/root",
                "/api/w/ws/drive/read-chunk?id=1",
                "/api/w/ws/drive/upload",
                "/api/w/ws/drive/requests/request-id",
                "/api/w/ws/%64rive/root",
                "/api/w/ws/tickets/../drive/root",
                "/api/w/ws/tickets/%2e%2e/drive/root",
                "/api/w/ws%2fdrive/root",
                "/api/w/ws/dr\tive/root",
                "/api/w/ws/dri\r\nve/root",
            ] {
                for method in [
                    WorkspaceRequestMethod::Get,
                    WorkspaceRequestMethod::Post,
                    WorkspaceRequestMethod::Put,
                    WorkspaceRequestMethod::Patch,
                    WorkspaceRequestMethod::Delete,
                ] {
                    let request = WorkspaceRequest::json(method, path, "{}");
                    for result in [
                        client.execute(request.clone()),
                        client.execute_with_timeout(request, Duration::from_secs(2)),
                    ] {
                        assert!(
                            matches!(result, Err(WorkspaceClientError::Unavailable(message)) if message.contains("Drive access denied")),
                            "path={path}, reviewer={reviewer}"
                        );
                    }
                    for result in [
                        client.execute_binary(binary(method, path)),
                        client.execute_binary_with_timeout(
                            binary(method, path),
                            Duration::from_secs(2),
                        ),
                    ] {
                        assert!(
                            matches!(result, Err(WorkspaceClientError::Unavailable(message)) if message.contains("Drive access denied")),
                            "path={path}, reviewer={reviewer}"
                        );
                    }
                }
            }
        }
        assert!(inner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn non_drive_requests_preserve_payloads_deadlines_and_special_operations() {
        let inner = Arc::new(RecordingClient::default());
        let client = InternalChildWorkspaceClient::new(inner.clone());
        let request = WorkspaceRequest::json(
            WorkspaceRequestMethod::Post,
            "/api/w/ws/tickets/query",
            "{\"query\":\"drive\"}",
        );
        let bytes = binary(WorkspaceRequestMethod::Put, "/api/w/ws/files/content");
        let timeout = Duration::from_millis(37);
        let expected = WorkspaceResponse {
            status: 202,
            body: "forwarded".into(),
        };
        assert_eq!(client.execute(request.clone()).unwrap(), expected);
        assert_eq!(
            client
                .execute_with_timeout(request.clone(), timeout)
                .unwrap(),
            expected
        );
        let expected_bytes = WorkspaceBinaryResponse {
            status: 206,
            body: vec![0, 255],
        };
        assert_eq!(
            client.execute_binary(bytes.clone()).unwrap(),
            expected_bytes
        );
        assert_eq!(
            client
                .execute_binary_with_timeout(bytes.clone(), timeout)
                .unwrap(),
            expected_bytes
        );
        assert!(
            matches!(client.current_prompt_projection(), Err(WorkspaceClientError::Request(message)) if message == "projection marker")
        );
        let discovery = WorkspaceWorkerDiscoveryRequest {
            cursor: Some("next".into()),
            limit: 3,
            query: Some("coder".into()),
        };
        assert!(
            matches!(client.list_workspace_workers(discovery.clone()), Err(WorkspaceClientError::Request(message)) if message == "discovery marker")
        );
        assert_eq!(
            client
                .execute_worker_remove("runtime", "worker", "reason")
                .unwrap(),
            expected
        );
        assert_eq!(
            client
                .execute_server_operation(WorkspaceServerOperation::WorkerControlList)
                .unwrap(),
            expected
        );
        assert_eq!(client.workspace_id(), Some("ws"));
        assert_eq!(client.kind(), "recording");
        assert!(client.is_available());
        assert_eq!(
            *inner.calls.lock().unwrap(),
            vec![
                Call::Json(request.clone(), None),
                Call::Json(request, Some(timeout)),
                Call::Binary(bytes.clone(), None),
                Call::Binary(bytes, Some(timeout)),
                Call::Projection,
                Call::Discovery(discovery),
                Call::Remove("runtime".into(), "worker".into(), "reason".into()),
                Call::Server,
            ]
        );
    }

    #[test]
    fn drive_restriction_preserves_reviewer_context_and_review_attestation() {
        let inner = Arc::new(RecordingClient::default());
        let context = ReviewerContext {
            ticket_id: "T1".into(),
            merge_request_id: "MR1".into(),
        };
        let client =
            InternalChildWorkspaceClient::new(Arc::new(ReviewerChildWorkspaceClient::new(
                inner.clone(),
                context.clone(),
                "trusted-token".into(),
            )));
        assert_eq!(client.reviewer_context(), Some(&context));
        assert_eq!(client.kind(), "runtime-reviewer-child");
        let request = WorkspaceRequest::json(
            WorkspaceRequestMethod::Post,
            "/api/w/ws/merge-requests/MR1/reviews",
            "{\"verdict\":\"approve\"}",
        );
        // Reviewer timeout execution uses its existing JSON attestation path, not raw parent access.
        client
            .execute_with_timeout(request, Duration::from_secs(1))
            .unwrap();
        let calls = inner.calls.lock().unwrap();
        let [Call::Json(request, None)] = calls.as_slice() else {
            panic!("unexpected calls: {calls:?}")
        };
        let body: serde_json::Value = serde_json::from_str(request.body.as_ref().unwrap()).unwrap();
        assert_eq!(body["capability_token"], "trusted-token");
        assert_eq!(body["verdict"], "approve");
        drop(calls);
        let count = inner.calls.lock().unwrap().len();
        assert!(
            client
                .execute(WorkspaceRequest::json(
                    WorkspaceRequestMethod::Delete,
                    "/api/w/ws/tickets/T1",
                    "{}"
                ))
                .is_err()
        );
        assert_eq!(inner.calls.lock().unwrap().len(), count);
    }
}
