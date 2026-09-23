use crate::BackendOrigin;
use std::fmt;
use std::time::Duration;

use server_api::{
    DeviceLoginPollRequest, DeviceLoginPollStatus, DeviceLoginStartRequest, RepositoryApiError,
    ServerApiClient,
};
pub use server_api::{DeviceLoginPollResponse, DeviceLoginStartResponse};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendAuthTarget {
    pub base_url: String,
}

impl BackendAuthTarget {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        let base_url = BackendOrigin::parse(&base_url)
            .map(|origin| origin.to_string())
            .unwrap_or(base_url);
        Self { base_url }
    }
}

#[derive(Debug)]
pub enum BackendAuthClientError {
    InvalidTarget(String),
    ServerApi(server_api::client_support::ClientError<RepositoryApiError>),
    BackendStatus { status: u16, body: String },
    MissingAccessToken,
}

impl fmt::Display for BackendAuthClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget(message) => f.write_str(message),
            Self::ServerApi(error) => write!(f, "{error}"),
            Self::BackendStatus { status, body } => {
                write!(f, "Backend auth returned HTTP {status}: {body}")
            }
            Self::MissingAccessToken => {
                f.write_str("Backend approved device login without an access token")
            }
        }
    }
}

impl std::error::Error for BackendAuthClientError {}

const AUTH_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

fn auth_client(target: &BackendAuthTarget) -> Result<ServerApiClient, BackendAuthClientError> {
    ServerApiClient::builder(&target.base_url)
        .map_err(|error| BackendAuthClientError::InvalidTarget(error.to_string()))?
        .response_body_limit(AUTH_RESPONSE_LIMIT)
        .build()
        .map_err(|error| BackendAuthClientError::InvalidTarget(error.to_string()))
}

pub async fn start_device_login(
    target: &BackendAuthTarget,
    client_name: Option<&str>,
) -> Result<DeviceLoginStartResponse, BackendAuthClientError> {
    auth_client(target)?
        .auth_device_login_start(DeviceLoginStartRequest {
            client_name: client_name.map(ToOwned::to_owned),
        })
        .await
        .map_err(BackendAuthClientError::ServerApi)
}

pub async fn poll_device_login(
    target: &BackendAuthTarget,
    device_code: &str,
) -> Result<DeviceLoginPollResponse, BackendAuthClientError> {
    auth_client(target)?
        .auth_device_login_poll(DeviceLoginPollRequest {
            device_code: device_code.to_string(),
        })
        .await
        .map_err(BackendAuthClientError::ServerApi)
}

fn device_login_poll_result(
    response: DeviceLoginPollResponse,
) -> Result<Option<String>, BackendAuthClientError> {
    match response.status {
        DeviceLoginPollStatus::Approved => response
            .access_token
            .ok_or(BackendAuthClientError::MissingAccessToken)
            .map(Some),
        DeviceLoginPollStatus::Expired => Err(BackendAuthClientError::BackendStatus {
            status: 410,
            body: "device login expired".to_string(),
        }),
        DeviceLoginPollStatus::Denied => Err(BackendAuthClientError::BackendStatus {
            status: 403,
            body: "device login was denied".to_string(),
        }),
        DeviceLoginPollStatus::Consumed => Err(BackendAuthClientError::BackendStatus {
            status: 409,
            body: "device login was already consumed".to_string(),
        }),
        DeviceLoginPollStatus::Pending => Ok(None),
    }
}

pub async fn wait_for_device_login(
    target: &BackendAuthTarget,
    device_code: &str,
    interval: Duration,
    expires_in: Duration,
) -> Result<String, BackendAuthClientError> {
    let started = std::time::Instant::now();
    loop {
        let response = poll_device_login(target, device_code).await?;
        if let Some(access_token) = device_login_poll_result(response)? {
            return Ok(access_token);
        }
        if started.elapsed() >= expires_in {
            return Err(BackendAuthClientError::BackendStatus {
                status: 408,
                body: "timed out waiting for device login approval".to_string(),
            });
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use server_api::DeviceAccessTokenType;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn poll_response(status: DeviceLoginPollStatus) -> DeviceLoginPollResponse {
        DeviceLoginPollResponse {
            status,
            access_token: None,
            token_type: None,
        }
    }

    #[tokio::test]
    async fn device_login_start_uses_generated_server_api_client() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = vec![0; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("POST /api/auth/device-login/start HTTP/1.1"));
            assert!(request.contains(r#"{"client_name":"yoi-cli"}"#));
            let body = r#"{"device_code":"device-secret","user_code":"ABCD-EFGH","verification_uri":"https://yoi.example/login/device","verification_uri_complete":"https://yoi.example/login/device?user_code=ABCD-EFGH","expires_in":600,"interval":5}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let response = start_device_login(&BackendAuthTarget::new(base_url), Some("yoi-cli"))
            .await
            .unwrap();
        assert_eq!(response.user_code, "ABCD-EFGH");
        handle.join().unwrap();
    }

    #[test]
    fn device_login_start_response_enforces_shared_expiry_bounds() {
        let valid = serde_json::json!({
            "device_code": "device-secret",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://yoi.example/login/device",
            "verification_uri_complete": "https://yoi.example/login/device?user_code=ABCD-EFGH",
            "expires_in": 600,
            "interval": 5
        });
        assert!(serde_json::from_value::<DeviceLoginStartResponse>(valid.clone()).is_ok());

        let mut expired = valid;
        expired["expires_in"] = serde_json::json!(0);
        assert!(serde_json::from_value::<DeviceLoginStartResponse>(expired).is_err());
    }

    #[test]
    fn device_login_poll_response_rejects_unknown_status() {
        assert!(
            serde_json::from_value::<DeviceLoginPollResponse>(
                serde_json::json!({"status": "future_status"}),
            )
            .is_err()
        );
    }

    #[test]
    fn device_login_poll_result_handles_pending_and_terminal_states() {
        assert!(
            device_login_poll_result(poll_response(DeviceLoginPollStatus::Pending))
                .unwrap()
                .is_none()
        );

        let approved = DeviceLoginPollResponse {
            status: DeviceLoginPollStatus::Approved,
            access_token: Some("access-secret".to_string()),
            token_type: Some(DeviceAccessTokenType::Bearer),
        };
        assert_eq!(
            device_login_poll_result(approved).unwrap(),
            Some("access-secret".to_string())
        );
        assert!(matches!(
            device_login_poll_result(poll_response(DeviceLoginPollStatus::Approved)),
            Err(BackendAuthClientError::MissingAccessToken)
        ));

        for (status, expected_http_status) in [
            (DeviceLoginPollStatus::Expired, 410),
            (DeviceLoginPollStatus::Denied, 403),
            (DeviceLoginPollStatus::Consumed, 409),
        ] {
            assert!(matches!(
                device_login_poll_result(poll_response(status)),
                Err(BackendAuthClientError::BackendStatus {
                    status,
                    ..
                }) if status == expected_http_status
            ));
        }
    }
}
