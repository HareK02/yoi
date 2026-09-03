use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode, Url, redirect};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

const TOKEN_FILE_NAME: &str = "backend-tokens.json";
const MAX_REDIRECTS: usize = 10;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BackendOrigin(String);

impl BackendOrigin {
    pub fn parse(input: &str) -> Result<Self, BackendApiClientError> {
        let url = Url::parse(input.trim()).map_err(|error| {
            BackendApiClientError::InvalidBackendOrigin(format!(
                "Backend URL is not a valid absolute URL: {error}"
            ))
        })?;
        if !url.path().bytes().all(|byte| byte == b'/')
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(BackendApiClientError::InvalidBackendOrigin(
                "Backend URL must contain only an origin, without a path, query, or fragment"
                    .to_string(),
            ));
        }
        Self::from_url(url)
    }

    fn from_url(mut url: Url) -> Result<Self, BackendApiClientError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(BackendApiClientError::InvalidBackendOrigin(
                "Backend URL scheme must be http or https".to_string(),
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(BackendApiClientError::InvalidBackendOrigin(
                "Backend URL must not contain user information".to_string(),
            ));
        }
        if url.host().is_none() {
            return Err(BackendApiClientError::InvalidBackendOrigin(
                "Backend URL must contain a host".to_string(),
            ));
        }
        let default_port = match url.scheme() {
            "http" => 80,
            "https" => 443,
            _ => unreachable!("validated Backend URL scheme"),
        };
        if url.port() == Some(default_port) {
            url.set_port(None).map_err(|()| {
                BackendApiClientError::InvalidBackendOrigin(
                    "Backend URL contains an invalid port".to_string(),
                )
            })?;
        }
        url.set_path("");
        url.set_query(None);
        url.set_fragment(None);
        let normalized = url.as_str().trim_end_matches('/').to_string();
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn url(&self, path_and_query: &str) -> Result<Url, BackendApiClientError> {
        if !path_and_query.starts_with('/') || path_and_query.starts_with("//") {
            return Err(BackendApiClientError::InvalidRequestPath(
                "Backend API request path must start with one `/`".to_string(),
            ));
        }
        Url::parse(&format!("{}{path_and_query}", self.0)).map_err(|error| {
            BackendApiClientError::InvalidRequestPath(format!(
                "Backend API request path is invalid: {error}"
            ))
        })
    }
}

impl fmt::Debug for BackendOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BackendOrigin").field(&self.0).finish()
    }
}

impl fmt::Display for BackendOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone)]
struct BackendAccessToken(String);

impl fmt::Debug for BackendAccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BackendAccessToken([REDACTED])")
    }
}

#[derive(Clone)]
pub struct BackendApiClient {
    origin: BackendOrigin,
    access_token: BackendAccessToken,
    asynchronous: reqwest::Client,
}

impl fmt::Debug for BackendApiClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackendApiClient")
            .field("origin", &self.origin)
            .field("access_token", &self.access_token)
            .finish_non_exhaustive()
    }
}

impl BackendApiClient {
    pub fn from_stored_token(base_url: &str) -> Result<Self, BackendApiClientError> {
        let path = backend_token_file_path()?;
        Self::from_token_file(base_url, &path)
    }

    fn from_token_file(base_url: &str, path: &Path) -> Result<Self, BackendApiClientError> {
        let origin = BackendOrigin::parse(base_url)?;
        let token_file = read_token_file(path)?;
        let entry = token_file.tokens.get(origin.as_str()).ok_or_else(|| {
            BackendApiClientError::TokenEntryMissing {
                origin: origin.clone(),
                path: path.to_path_buf(),
            }
        })?;
        validate_token_entry(entry, &origin, path)?;
        Self::new(origin, BackendAccessToken(entry.access_token.clone()))
    }

    fn new(
        origin: BackendOrigin,
        access_token: BackendAccessToken,
    ) -> Result<Self, BackendApiClientError> {
        let asynchronous = reqwest::Client::builder()
            .redirect(redirect_policy(origin.clone()))
            .build()
            .map_err(BackendApiClientError::Http)?;
        Ok(Self {
            origin,
            access_token,
            asynchronous,
        })
    }

    pub fn origin(&self) -> &BackendOrigin {
        &self.origin
    }

    pub fn request(
        &self,
        method: Method,
        path_and_query: &str,
    ) -> Result<reqwest::RequestBuilder, BackendApiClientError> {
        let url = self.origin.url(path_and_query)?;
        Ok(self
            .asynchronous
            .request(method, url)
            .bearer_auth(&self.access_token.0))
    }

    pub fn blocking_request(
        &self,
        method: Method,
        path_and_query: &str,
    ) -> Result<reqwest::blocking::RequestBuilder, BackendApiClientError> {
        let url = self.origin.url(path_and_query)?;
        let client = reqwest::blocking::Client::builder()
            .redirect(redirect_policy(self.origin.clone()))
            .build()
            .map_err(BackendApiClientError::Http)?;
        Ok(client
            .request(method, url)
            .bearer_auth(&self.access_token.0))
    }

    pub(crate) fn authorization_header_value(&self) -> String {
        format!("Bearer {}", self.access_token.0)
    }

    pub async fn require_success(
        &self,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, BackendApiClientError> {
        let status = response.status();
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                self.check_status(status)?;
            }
            status if !status.is_success() => {
                let detail = response
                    .bytes()
                    .await
                    .ok()
                    .and_then(|body| backend_error_detail(&body));
                return Err(BackendApiClientError::BackendResponse {
                    origin: self.origin.clone(),
                    status: status.as_u16(),
                    detail,
                });
            }
            _ => {}
        }
        Ok(response)
    }

    pub fn check_status(&self, status: StatusCode) -> Result<(), BackendApiClientError> {
        match status {
            StatusCode::UNAUTHORIZED => Err(BackendApiClientError::Unauthorized {
                origin: self.origin.clone(),
            }),
            StatusCode::FORBIDDEN => Err(BackendApiClientError::Forbidden {
                origin: self.origin.clone(),
            }),
            status if !status.is_success() => Err(BackendApiClientError::BackendStatus {
                origin: self.origin.clone(),
                status: status.as_u16(),
            }),
            _ => Ok(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_access_token_for_test(
        base_url: &str,
        access_token: &str,
    ) -> Result<Self, BackendApiClientError> {
        Self::new(
            BackendOrigin::parse(base_url)?,
            BackendAccessToken(access_token.to_string()),
        )
    }
}

fn redirect_policy(origin: BackendOrigin) -> redirect::Policy {
    redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error("Backend request exceeded the redirect limit");
        }
        match BackendOrigin::from_url(attempt.url().clone()) {
            Ok(target_origin) if target_origin == origin => attempt.follow(),
            Ok(target_origin) => attempt.error(format!(
                "Backend request refused a cross-origin redirect from {origin} to {target_origin}"
            )),
            Err(error) => attempt.error(error.to_string()),
        }
    })
}

#[derive(Deserialize)]
struct BackendErrorBody {
    message: String,
}

fn backend_error_detail(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<BackendErrorBody>(body)
        .ok()
        .map(|body| body.message)
        .filter(|message| !message.trim().is_empty())
}

#[derive(Debug)]
pub enum BackendApiClientError {
    InvalidBackendOrigin(String),
    InvalidRequestPath(String),
    ConfigDirectoryUnavailable,
    TokenFileMissing {
        path: PathBuf,
    },
    TokenFileMalformed {
        path: PathBuf,
        message: String,
    },
    TokenEntryMissing {
        origin: BackendOrigin,
        path: PathBuf,
    },
    TokenExpired {
        origin: BackendOrigin,
        expired_at: String,
    },
    Http(reqwest::Error),
    Unauthorized {
        origin: BackendOrigin,
    },
    Forbidden {
        origin: BackendOrigin,
    },
    BackendStatus {
        origin: BackendOrigin,
        status: u16,
    },
    BackendResponse {
        origin: BackendOrigin,
        status: u16,
        detail: Option<String>,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for BackendApiClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBackendOrigin(message) | Self::InvalidRequestPath(message) => {
                f.write_str(message)
            }
            Self::ConfigDirectoryUnavailable => f.write_str(
                "cannot locate the client configuration directory for backend-tokens.json",
            ),
            Self::TokenFileMissing { path } => write!(
                f,
                "Backend token file {} is missing; run `yoi login --backend <BACKEND>` first",
                path.display()
            ),
            Self::TokenFileMalformed { path, message } => write!(
                f,
                "Backend token file {} is malformed: {message}; run `yoi login --backend <BACKEND>` again",
                path.display()
            ),
            Self::TokenEntryMissing { origin, path } => write!(
                f,
                "no Backend token for {origin} exists in {}; login URLs are matched by normalized origin, so run `yoi login --backend {origin}`",
                path.display()
            ),
            Self::TokenExpired { origin, expired_at } => write!(
                f,
                "Backend token for {origin} expired at {expired_at}; run `yoi login --backend {origin}` again"
            ),
            Self::Http(error) => write!(f, "Backend request failed: {error}"),
            Self::Unauthorized { origin } => write!(
                f,
                "Backend {origin} returned HTTP 401 for the saved token; it may be expired or revoked, so run `yoi login --backend {origin}` again"
            ),
            Self::Forbidden { origin } => write!(
                f,
                "Backend {origin} returned HTTP 403; the saved token is authenticated but is not authorized for this operation"
            ),
            Self::BackendStatus { origin, status } => {
                write!(f, "Backend {origin} returned HTTP {status}")
            }
            Self::BackendResponse {
                origin,
                status,
                detail,
            } => {
                write!(f, "Backend {origin} returned HTTP {status}")?;
                if let Some(detail) = detail {
                    write!(f, ": {detail}")?;
                }
                Ok(())
            }
            Self::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for BackendApiClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BackendTokenFile {
    tokens: BTreeMap<String, BackendTokenEntry>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BackendTokenEntry {
    token_type: String,
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
}

pub fn save_backend_token(
    base_url: &str,
    token_type: &str,
    access_token: &str,
) -> Result<PathBuf, BackendApiClientError> {
    save_backend_token_with_expiry(base_url, token_type, access_token, None)
}

fn save_backend_token_with_expiry(
    base_url: &str,
    token_type: &str,
    access_token: &str,
    expires_at: Option<String>,
) -> Result<PathBuf, BackendApiClientError> {
    let path = backend_token_file_path()?;
    save_backend_token_to_file(base_url, token_type, access_token, expires_at, &path)?;
    Ok(path)
}

fn save_backend_token_to_file(
    base_url: &str,
    token_type: &str,
    access_token: &str,
    expires_at: Option<String>,
    path: &Path,
) -> Result<(), BackendApiClientError> {
    let origin = BackendOrigin::parse(base_url)?;
    let mut token_file = if path.exists() {
        read_token_file(&path)?
    } else {
        BackendTokenFile {
            tokens: BTreeMap::new(),
        }
    };
    let entry = BackendTokenEntry {
        token_type: token_type.to_string(),
        access_token: access_token.to_string(),
        expires_at,
    };
    validate_token_entry(&entry, &origin, path)?;
    token_file.tokens.insert(origin.to_string(), entry);
    write_token_file(path, &token_file)?;
    Ok(())
}

pub fn backend_token_file_path() -> Result<PathBuf, BackendApiClientError> {
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(config_home).join("yoi").join(TOKEN_FILE_NAME));
    }
    let Some(home) = env::var_os("HOME") else {
        return Err(BackendApiClientError::ConfigDirectoryUnavailable);
    };
    Ok(PathBuf::from(home)
        .join(".config")
        .join("yoi")
        .join(TOKEN_FILE_NAME))
}

fn read_token_file(path: &Path) -> Result<BackendTokenFile, BackendApiClientError> {
    let bytes = fs::read(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            BackendApiClientError::TokenFileMissing {
                path: path.to_path_buf(),
            }
        } else {
            BackendApiClientError::Io {
                path: path.to_path_buf(),
                source,
            }
        }
    })?;
    let raw: BackendTokenFile = serde_json::from_slice(&bytes).map_err(|error| {
        BackendApiClientError::TokenFileMalformed {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    normalize_token_file(raw, path)
}

fn normalize_token_file(
    token_file: BackendTokenFile,
    path: &Path,
) -> Result<BackendTokenFile, BackendApiClientError> {
    let mut normalized = BTreeMap::new();
    for (raw_origin, entry) in token_file.tokens {
        let origin = BackendOrigin::parse(&raw_origin).map_err(|error| {
            BackendApiClientError::TokenFileMalformed {
                path: path.to_path_buf(),
                message: format!("token key `{raw_origin}` is invalid: {error}"),
            }
        })?;
        if normalized.insert(origin.to_string(), entry).is_some() {
            return Err(BackendApiClientError::TokenFileMalformed {
                path: path.to_path_buf(),
                message: format!("more than one token entry normalizes to `{origin}`"),
            });
        }
    }
    Ok(BackendTokenFile { tokens: normalized })
}

fn validate_token_entry(
    entry: &BackendTokenEntry,
    origin: &BackendOrigin,
    path: &Path,
) -> Result<(), BackendApiClientError> {
    if !entry.token_type.eq_ignore_ascii_case("Bearer") {
        return Err(BackendApiClientError::TokenFileMalformed {
            path: path.to_path_buf(),
            message: format!("token for `{origin}` does not use the Bearer token type"),
        });
    }
    if entry.access_token.trim().is_empty()
        || entry.access_token.contains('\r')
        || entry.access_token.contains('\n')
    {
        return Err(BackendApiClientError::TokenFileMalformed {
            path: path.to_path_buf(),
            message: format!("token for `{origin}` is empty or contains an invalid line break"),
        });
    }
    if reqwest::header::HeaderValue::from_str(&format!("Bearer {}", entry.access_token)).is_err() {
        return Err(BackendApiClientError::TokenFileMalformed {
            path: path.to_path_buf(),
            message: format!("token for `{origin}` cannot be represented as an HTTP header"),
        });
    }
    if let Some(expires_at) = entry.expires_at.as_deref() {
        let expiration = DateTime::parse_from_rfc3339(expires_at).map_err(|error| {
            BackendApiClientError::TokenFileMalformed {
                path: path.to_path_buf(),
                message: format!("token for `{origin}` has invalid expires_at: {error}"),
            }
        })?;
        if expiration <= Utc::now() {
            return Err(BackendApiClientError::TokenExpired {
                origin: origin.clone(),
                expired_at: expires_at.to_string(),
            });
        }
    }
    Ok(())
}

fn write_token_file(
    path: &Path,
    token_file: &BackendTokenFile,
) -> Result<(), BackendApiClientError> {
    let parent = path
        .parent()
        .ok_or(BackendApiClientError::ConfigDirectoryUnavailable)?;
    fs::create_dir_all(parent).map_err(|source| BackendApiClientError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let payload = serde_json::to_vec_pretty(token_file).map_err(|error| {
        BackendApiClientError::TokenFileMalformed {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    let temp_path = parent.join(format!(".{TOKEN_FILE_NAME}.tmp-{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp_path)
        .map_err(|source| BackendApiClientError::Io {
            path: temp_path.clone(),
            source,
        })?;
    file.write_all(&payload)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|source| BackendApiClientError::Io {
            path: temp_path.clone(),
            source,
        })?;
    fs::rename(&temp_path, path).map_err(|source| BackendApiClientError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!(
            "yoi-client-{label}-{}-{nonce}.json",
            std::process::id()
        ))
    }

    fn write_fixture(path: &Path, value: serde_json::Value) {
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    #[test]
    fn backend_origin_normalizes_safe_equivalents() {
        let variants = [
            "HTTP://Example.COM",
            "http://example.com/",
            "http://example.com:80////",
        ];
        for variant in variants {
            assert_eq!(
                BackendOrigin::parse(variant).unwrap().as_str(),
                "http://example.com"
            );
        }
        assert_eq!(
            BackendOrigin::parse("https://EXAMPLE.com:443/")
                .unwrap()
                .as_str(),
            "https://example.com"
        );
        assert_eq!(
            BackendOrigin::parse("https://example.com:8443/")
                .unwrap()
                .as_str(),
            "https://example.com:8443"
        );
    }

    #[test]
    fn backend_error_detail_preserves_public_server_message() {
        let detail = backend_error_detail(
            br#"{"error":"Bad Request","message":"working_directory_runtime_mismatch: Working directory is owned by a different Runtime","diagnostics":[{"code":"working_directory_runtime_mismatch"}]}"#,
        );
        let error = BackendApiClientError::BackendResponse {
            origin: BackendOrigin::parse("http://127.0.0.1:8787").unwrap(),
            status: 400,
            detail,
        };

        assert_eq!(
            error.to_string(),
            "Backend http://127.0.0.1:8787 returned HTTP 400: working_directory_runtime_mismatch: Working directory is owned by a different Runtime"
        );
    }

    #[test]
    fn backend_origin_rejects_unsafe_authority_changes() {
        for invalid in [
            "ftp://example.com",
            "https://user@example.com",
            "https://example.com/api",
            "https://example.com/?query=1",
            "https://example.com/#fragment",
        ] {
            assert!(BackendOrigin::parse(invalid).is_err(), "accepted {invalid}");
        }
        assert_ne!(
            BackendOrigin::parse("http://localhost:8787").unwrap(),
            BackendOrigin::parse("http://127.0.0.1:8787").unwrap()
        );
    }

    #[test]
    fn token_lookup_distinguishes_missing_malformed_mismatch_and_expired() {
        let missing = temp_path("missing");
        assert!(matches!(
            BackendApiClient::from_token_file("http://localhost:8787", &missing),
            Err(BackendApiClientError::TokenFileMissing { .. })
        ));

        let malformed = temp_path("malformed");
        fs::write(&malformed, b"not json").unwrap();
        assert!(matches!(
            BackendApiClient::from_token_file("http://localhost:8787", &malformed),
            Err(BackendApiClientError::TokenFileMalformed { .. })
        ));

        let mismatch = temp_path("mismatch");
        write_fixture(
            &mismatch,
            serde_json::json!({"tokens": {"http://localhost:8787": {
                "token_type": "Bearer", "access_token": "secret"
            }}}),
        );
        assert!(matches!(
            BackendApiClient::from_token_file("http://127.0.0.1:8787", &mismatch),
            Err(BackendApiClientError::TokenEntryMissing { .. })
        ));

        let expired = temp_path("expired");
        write_fixture(
            &expired,
            serde_json::json!({"tokens": {"http://localhost:8787": {
                "token_type": "Bearer",
                "access_token": "secret",
                "expires_at": "2000-01-01T00:00:00Z"
            }}}),
        );
        assert!(matches!(
            BackendApiClient::from_token_file("http://localhost:8787", &expired),
            Err(BackendApiClientError::TokenExpired { .. })
        ));

        for path in [malformed, mismatch, expired] {
            let _ = fs::remove_file(path);
        }
    }

    #[test]
    fn token_write_and_lookup_share_origin_normalization() {
        let path = temp_path("normalized-write");
        save_backend_token_to_file(
            "HTTP://Example.COM:80////",
            "Bearer",
            "normalized-secret",
            None,
            &path,
        )
        .unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\"http://example.com\""));
        let client = BackendApiClient::from_token_file("http://example.com/", &path).unwrap();
        assert_eq!(
            client.authorization_header_value(),
            "Bearer normalized-secret"
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn client_debug_and_errors_never_include_token_value() {
        let client = BackendApiClient::from_access_token_for_test(
            "http://localhost:8787",
            "never-print-this-token",
        )
        .unwrap();
        assert!(!format!("{client:?}").contains("never-print-this-token"));
        assert!(
            !BackendApiClientError::Unauthorized {
                origin: client.origin().clone()
            }
            .to_string()
            .contains("never-print-this-token")
        );
    }

    #[test]
    fn authenticated_requests_follow_only_same_origin_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            for response in [
                "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = vec![0; 4096];
                let read = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
                assert!(request.contains("authorization: bearer redirect-secret\r\n"));
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let client =
            BackendApiClient::from_access_token_for_test(&origin, "redirect-secret").unwrap();
        let response = client
            .blocking_request(Method::GET, "/start")
            .unwrap()
            .send()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        handle.join().unwrap();
    }

    #[test]
    fn authenticated_requests_reject_cross_origin_redirects_without_leaking_token() {
        let source = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let source_origin = format!("http://{}", source.local_addr().unwrap());
        let target_origin = format!("http://{}", target.local_addr().unwrap());
        let location = format!("{target_origin}/capture");
        let handle = thread::spawn(move || {
            let (mut stream, _) = source.accept().unwrap();
            let mut request = vec![0; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            assert!(request.contains("authorization: bearer redirect-secret\r\n"));
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let client =
            BackendApiClient::from_access_token_for_test(&source_origin, "redirect-secret")
                .unwrap();
        let error = client
            .blocking_request(Method::GET, "/start")
            .unwrap()
            .send()
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("redirect"));
        assert!(!message.contains("redirect-secret"));
        handle.join().unwrap();
        thread::sleep(Duration::from_millis(20));
        assert!(matches!(
            target.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }

    #[test]
    fn status_diagnostics_distinguish_unauthorized_and_forbidden() {
        let client =
            BackendApiClient::from_access_token_for_test("http://localhost:8787", "secret")
                .unwrap();
        assert!(matches!(
            client.check_status(StatusCode::UNAUTHORIZED),
            Err(BackendApiClientError::Unauthorized { .. })
        ));
        assert!(matches!(
            client.check_status(StatusCode::FORBIDDEN),
            Err(BackendApiClientError::Forbidden { .. })
        ));
    }
}
