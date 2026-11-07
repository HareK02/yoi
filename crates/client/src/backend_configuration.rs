//! Backend credentials and HTTP policy for the generated configuration adoption client.
//!
//! Source, preview, adoption, and reconciliation contracts remain owned by `server_api`;
//! this adapter does not interpret adoption outcomes or retry requests.

use crate::backend_workspace::{SERVER_RESPONSE_LIMIT, ServerBearerAuthorizer};
use crate::{BackendApiClient, BackendWorkspaceClientError};

pub fn configuration_adoption_client(
    backend: &BackendApiClient,
) -> Result<
    server_api::ConfigurationAdoptionApiClient<impl server_api::client_support::RequestAuthorizer>,
    BackendWorkspaceClientError,
> {
    server_api::ConfigurationAdoptionApiClient::builder(backend.origin().as_str())
        .map_err(|error| BackendWorkspaceClientError::InvalidTarget(error.to_string()))?
        .client(backend.asynchronous_client())
        .authorizer(ServerBearerAuthorizer::new(backend))
        .response_body_limit(SERVER_RESPONSE_LIMIT)
        .build()
        .map_err(|error| BackendWorkspaceClientError::InvalidTarget(error.to_string()))
}

#[cfg(test)]
mod tests {
    //! The production factory and generated request codec connect to a loopback HTTP fixture.
    //! These tests cover route, credential, and redirect wiring, not Server owner authorization.

    use super::*;
    use server_api::client_support::{
        AuthorizerRequest, ClientError, ClientFailure, RequestAuthorizer,
    };
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::timeout;

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);
    const TOKEN: &str = "configuration-secret";

    async fn respond(listener: TcpListener, response: String) -> String {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut chunk = [0; 1024];
            let read = stream.read(&mut chunk).await.unwrap();
            assert_ne!(read, 0, "request ended before HTTP headers");
            request.extend_from_slice(&chunk[..read]);
            assert!(request.len() < 16 * 1024, "unexpectedly large request");
        }
        stream.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap().to_ascii_lowercase()
    }

    #[tokio::test]
    async fn source_uses_generated_route_and_backend_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let backend = BackendApiClient::from_access_token_for_test(&origin, TOKEN).unwrap();
        let client = configuration_adoption_client(&backend).unwrap();
        let body = serde_json::to_string(&server_api::RepositoryApiError::new(
            403,
            "Forbidden",
            "owner required",
            Vec::new(),
        ))
        .unwrap();
        let response = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );

        let (result, request) = timeout(TEST_TIMEOUT, async {
            tokio::join!(
                client.configuration_source("workspace-test".to_owned()),
                respond(listener, response)
            )
        })
        .await
        .unwrap();

        assert!(request.starts_with("get /api/w/workspace-test/config/adoption "));
        assert!(request.contains(&format!("authorization: bearer {TOKEN}\r\n")));
        assert!(matches!(
            result,
            Err(ClientError::Public { status: reqwest::StatusCode::FORBIDDEN, error })
                if error.legacy().message == "owner required"
        ));
    }

    #[tokio::test]
    async fn cross_origin_redirect_is_rejected_without_sending_credentials() {
        let source = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", source.local_addr().unwrap());
        let backend = BackendApiClient::from_access_token_for_test(&origin, TOKEN).unwrap();
        let client = configuration_adoption_client(&backend).unwrap();
        let response = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://{}/capture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            target.local_addr().unwrap()
        );

        let (result, _) = timeout(TEST_TIMEOUT, async {
            tokio::join!(
                client.configuration_source("workspace-test".to_owned()),
                respond(source, response)
            )
        })
        .await
        .unwrap();

        let error = result.unwrap_err();
        assert!(matches!(
            error,
            ClientError::Failure(ClientFailure::Transport)
        ));
        assert!(!format!("{error:?} {error}").contains(TOKEN));
        assert!(
            matches!(target.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn bearer_authorizer_redacts_credentials_in_debug_output() {
        let backend =
            BackendApiClient::from_access_token_for_test("https://backend.example.test", TOKEN)
                .unwrap();
        let authorizer = ServerBearerAuthorizer::new(&backend);
        let headers = authorizer
            .authorize(AuthorizerRequest {
                method: &reqwest::Method::GET,
                path_and_query: "/api/w/workspace-test/config/adoption",
                body: &[],
            })
            .unwrap();

        assert!(headers[reqwest::header::AUTHORIZATION].is_sensitive());
        assert!(!format!("{authorizer:?} {headers:?}").contains(TOKEN));
    }
}
