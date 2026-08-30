use crate::BackendOrigin;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

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

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DeviceLoginStartResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DeviceLoginPollResponse {
    pub status: String,
    pub access_token: Option<String>,
    pub token_type: Option<String>,
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

#[derive(Debug, Serialize)]
struct DeviceLoginStartRequest<'a> {
    client_name: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct DeviceLoginPollRequest<'a> {
    device_code: &'a str,
}

pub async fn start_device_login(
    target: &BackendAuthTarget,
    client_name: Option<&str>,
) -> Result<DeviceLoginStartResponse, BackendAuthClientError> {
    let client = reqwest::Client::new();
    let response = client
        .post(target.api_url("/api/auth/device-login/start"))
        .json(&DeviceLoginStartRequest { client_name })
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
        .json(&DeviceLoginPollRequest { device_code })
        .send()
        .await?;
    parse_json_response(response).await
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
        match response.status.as_str() {
            "approved" => {
                return response
                    .access_token
                    .ok_or(BackendAuthClientError::MissingAccessToken);
            }
            "expired" => {
                return Err(BackendAuthClientError::BackendStatus {
                    status: 410,
                    body: "device login expired".to_string(),
                });
            }
            "consumed" => {
                return Err(BackendAuthClientError::BackendStatus {
                    status: 409,
                    body: "device login was already consumed".to_string(),
                });
            }
            _ => {}
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
