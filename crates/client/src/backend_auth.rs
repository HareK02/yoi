use crate::BackendOrigin;
use serde::Deserialize;
use std::fmt;
use std::time::Duration;

use workspace_api::{DeviceLoginPollRequest, DeviceLoginPollStatus, DeviceLoginStartRequest};
pub use workspace_api::{DeviceLoginPollResponse, DeviceLoginStartResponse};

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

    fn api_url(&self, path: &str) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            path.strip_prefix('/')
                .map(|path| format!("/{path}"))
                .unwrap_or_else(|| path.to_string())
        )
    }
}

#[derive(Debug)]
pub enum BackendAuthClientError {
    Http(reqwest::Error),
    BackendStatus { status: u16, body: String },
    MissingAccessToken,
}

impl fmt::Display for BackendAuthClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(f, "Backend auth request failed: {error}"),
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

impl From<reqwest::Error> for BackendAuthClientError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

pub async fn start_device_login(
    target: &BackendAuthTarget,
    client_name: Option<&str>,
) -> Result<DeviceLoginStartResponse, BackendAuthClientError> {
    let client = reqwest::Client::new();
    let response = client
        .post(target.api_url("/api/auth/device-login/start"))
        .json(&DeviceLoginStartRequest {
            client_name: client_name.map(ToOwned::to_owned),
        })
        .send()
        .await?;
    parse_json_response(response).await
}

pub async fn poll_device_login(
    target: &BackendAuthTarget,
    device_code: &str,
) -> Result<DeviceLoginPollResponse, BackendAuthClientError> {
    let client = reqwest::Client::new();
    let response = client
        .post(target.api_url("/api/auth/device-login/poll"))
        .json(&DeviceLoginPollRequest {
            device_code: device_code.to_string(),
        })
        .send()
        .await?;
    parse_json_response(response).await
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

async fn parse_json_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, BackendAuthClientError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(BackendAuthClientError::BackendStatus {
            status: status.as_u16(),
            body,
        });
    }
    Ok(response.json::<T>().await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use workspace_api::DeviceAccessTokenType;

    fn poll_response(status: DeviceLoginPollStatus) -> DeviceLoginPollResponse {
        DeviceLoginPollResponse {
            status,
            access_token: None,
            token_type: None,
        }
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
