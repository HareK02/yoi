//! `HttpTransport<S: Scheme>`: すべての LLM wire scheme を共通の 1 本の
//! HTTP クライアントで扱う。
//!
//! 旧 `providers/{anthropic,openai,gemini,ollama}.rs` を置き換える。
//! scheme 固有の差分は [`Scheme`] trait 実装に委譲する。

use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt, TryStreamExt};
use reqwest::header::{
    ACCEPT, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue,
    RETRY_AFTER, TRANSFER_ENCODING,
};
use serde_json::{Map, Value, json};

use super::auth::{AuthProvider, AuthRequirement};
use super::capability::ModelCapability;
use super::client::{ConfigWarning, LlmClient, ResponseStream};
use super::error::ClientError;
use super::event::Event;
use super::scheme::Scheme;
use super::types::{Request, RequestConfig};

pub const DEFAULT_STREAM_OPEN_TIMEOUT: Duration = Duration::from_secs(20);
pub const DEFAULT_FIRST_STREAM_EVENT_TIMEOUT: Duration = Duration::from_secs(30);

/// `AuthRef` を解決したランタイム表現。`crates/provider` が構築する。
///
/// - `None`: 認証ヘッダを送らない（Ollama 等の opt-out）
/// - `ApiKey`: 静的な API key 文字列
/// - `Custom`: リクエスト毎に動的にヘッダを組み立てる（Codex OAuth 等）
#[derive(Debug, Clone)]
pub enum ResolvedAuth {
    None,
    ApiKey(String),
    Custom(Arc<dyn AuthProvider>),
}

impl ResolvedAuth {
    /// 認証要件と実際の解決値が噛み合うか検査する。構築時検証用。
    ///
    /// - `ResolvedAuth::None` は認証を付けない宣言なので、どの
    ///   `AuthRequirement` でも受け入れる（Ollama の Anthropic scheme
    ///   流用は `required_auth = XApiKey` だが認証ヘッダなしで動く）
    /// - `ResolvedAuth::Custom` は「ヘッダ組立を全部こちらで行う」
    ///   宣言なので、scheme が要求する形式によらず受け入れる
    pub fn matches(&self, req: AuthRequirement) -> bool {
        match (self, req) {
            (Self::None, _) => true,
            (Self::Custom(_), _) => true,
            (
                Self::ApiKey(_),
                AuthRequirement::Bearer
                | AuthRequirement::XApiKey
                | AuthRequirement::QueryParam { .. },
            ) => true,
            _ => false,
        }
    }
}

fn header_value_for_diagnostics(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn response_header_diagnostics(headers: &HeaderMap) -> serde_json::Value {
    serde_json::json!({
        "content_type": header_value_for_diagnostics(headers, CONTENT_TYPE.as_str()),
        "content_encoding": header_value_for_diagnostics(headers, CONTENT_ENCODING.as_str()),
        "transfer_encoding": header_value_for_diagnostics(headers, TRANSFER_ENCODING.as_str()),
        "content_length": header_value_for_diagnostics(headers, CONTENT_LENGTH.as_str()),
    })
}

fn request_header_diagnostics(headers: &HeaderMap) -> serde_json::Value {
    serde_json::json!({
        "content_type": header_value_for_diagnostics(headers, CONTENT_TYPE.as_str()),
        "content_encoding": header_value_for_diagnostics(headers, CONTENT_ENCODING.as_str()),
        "accept": header_value_for_diagnostics(headers, ACCEPT.as_str()),
        "openai_beta": header_value_for_diagnostics(headers, "openai-beta"),
        "session_id_present": headers.contains_key("session-id"),
        "thread_id_present": headers.contains_key("thread-id"),
        "legacy_session_id_present": headers.contains_key("session_id"),
        "legacy_thread_id_present": headers.contains_key("thread_id"),
        "x_client_request_id_present": headers.contains_key("x-client-request-id"),
        "chatgpt_account_id_present": headers.contains_key("chatgpt-account-id"),
    })
}

fn sse_error_context(status: u16, headers: &serde_json::Value, source: &str) -> String {
    let field = |name: &str| {
        headers
            .get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<none>")
    };
    format!(
        "SSE stream parse failed after HTTP {status}: {source}; content-type={}, content-encoding={}, transfer-encoding={}, content-length={}",
        field("content_type"),
        field("content_encoding"),
        field("transfer_encoding"),
        field("content_length")
    )
}

/// scheme 共通の HTTP 通信層。
pub struct HttpTransport<S: Scheme> {
    http_client: reqwest::Client,
    scheme: S,
    model_id: String,
    base_url: String,
    auth: ResolvedAuth,
    capability: ModelCapability,
}

impl<S: Scheme> HttpTransport<S> {
    /// 新しい transport を作る。`base_url` は末尾スラッシュの有無を
    /// どちらでも受け付ける（内部で正規化）。
    pub fn new(
        scheme: S,
        model_id: impl Into<String>,
        base_url: impl Into<String>,
        auth: ResolvedAuth,
        capability: ModelCapability,
    ) -> Self {
        let base_url = base_url.into();
        let base_url = base_url.trim_end_matches('/').to_string();
        Self {
            http_client: reqwest::Client::new(),
            scheme,
            model_id: model_id.into(),
            base_url,
            auth,
            capability,
        }
    }

    /// カスタム HTTP クライアントを差し込む（テスト等）。
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = client;
        self
    }

    fn build_url(&self) -> String {
        let path = self.scheme.path(&self.model_id);
        let url = format!("{}{}", self.base_url, path);
        // Gemini のようにクエリパラメータで認証する場合は URL にキーを追記する
        if let (AuthRequirement::QueryParam { name }, ResolvedAuth::ApiKey(key)) =
            (self.scheme.required_auth(), &self.auth)
        {
            let sep = if url.contains('?') { '&' } else { '?' };
            format!("{url}{sep}{name}={key}")
        } else {
            url
        }
    }

    async fn build_headers(&self) -> Result<HeaderMap, ClientError> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        match (&self.auth, self.scheme.required_auth()) {
            (ResolvedAuth::None, _) | (_, AuthRequirement::None) => {}
            (ResolvedAuth::Custom(provider), _) => {
                for (name, mut value) in provider.headers().await? {
                    value.set_sensitive(true);
                    headers.insert(name, value);
                }
            }
            (ResolvedAuth::ApiKey(key), AuthRequirement::Bearer) => {
                let mut val = HeaderValue::from_str(&format!("Bearer {key}"))
                    .map_err(|e| ClientError::Config(format!("invalid api key: {e}")))?;
                val.set_sensitive(true);
                headers.insert("Authorization", val);
            }
            (ResolvedAuth::ApiKey(key), AuthRequirement::XApiKey) => {
                let mut val = HeaderValue::from_str(key.as_str())
                    .map_err(|e| ClientError::Config(format!("invalid api key: {e}")))?;
                val.set_sensitive(true);
                headers.insert("x-api-key", val);
            }
            (_, AuthRequirement::QueryParam { .. }) => {
                // クエリパラメータは `build_url` で付与済み
            }
            (ResolvedAuth::ApiKey(_), AuthRequirement::Custom) => {
                // scheme が Custom を要求する組合せに ApiKey は流れてこない想定
                // （`matches()` で弾かれる）。安全側で何もしない
            }
        }

        for (name, value) in self.scheme.additional_headers() {
            let hv = HeaderValue::from_str(&value)
                .map_err(|e| ClientError::Config(format!("invalid header {name}: {e}")))?;
            headers.insert(name, hv);
        }

        Ok(headers)
    }

    fn is_codex_backend(&self) -> bool {
        match &self.auth {
            ResolvedAuth::Custom(provider) => provider.is_codex_backend(),
            _ => false,
        }
    }

    fn apply_stream_headers(
        &self,
        headers: &mut HeaderMap,
        request: &Request,
    ) -> Result<(), ClientError> {
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));

        if self.is_codex_backend()
            && let Some(cache_key) = request.cache_key.as_deref()
        {
            let value = HeaderValue::from_str(cache_key).map_err(|e| {
                ClientError::Config(format!("invalid Codex conversation header: {e}"))
            })?;
            // Codex CLI sends hyphenated session/thread headers to the
            // ChatGPT Codex backend. Keep the legacy underscore header for
            // existing traces/backends while exposing the current Codex shape.
            headers.insert(HeaderName::from_static("session-id"), value.clone());
            headers.insert(HeaderName::from_static("thread-id"), value.clone());
            headers.insert(HeaderName::from_static("session_id"), value.clone());
            headers.insert(HeaderName::from_static("x-client-request-id"), value);
        }

        Ok(())
    }

    fn encode_request_body(
        &self,
        body: &serde_json::Value,
        headers: &mut HeaderMap,
    ) -> Result<RequestBody, ClientError> {
        if !self.is_codex_backend() {
            return Ok(RequestBody::Json(body.clone()));
        }

        let raw = serde_json::to_vec(body)?;
        let raw_json_bytes = raw.len();
        let compressed = zstd::stream::encode_all(std::io::Cursor::new(raw), 3)
            .map_err(|e| ClientError::Config(format!("failed to zstd-compress request: {e}")))?;
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("zstd"));
        Ok(RequestBody::CompressedJson {
            bytes: compressed,
            raw_json_bytes,
        })
    }
}

enum RequestBody {
    Json(serde_json::Value),
    CompressedJson {
        bytes: Vec<u8>,
        raw_json_bytes: usize,
    },
}

impl RequestBody {
    fn encoding(&self) -> &'static str {
        match self {
            Self::Json(_) => "json",
            Self::CompressedJson { .. } => "zstd",
        }
    }

    fn raw_json_bytes(&self) -> Option<usize> {
        match self {
            Self::Json(body) => serde_json::to_vec(body).ok().map(|bytes| bytes.len()),
            Self::CompressedJson { raw_json_bytes, .. } => Some(*raw_json_bytes),
        }
    }

    fn wire_bytes(&self) -> Option<usize> {
        match self {
            Self::Json(body) => serde_json::to_vec(body).ok().map(|bytes| bytes.len()),
            Self::CompressedJson { bytes, .. } => Some(bytes.len()),
        }
    }
}

fn auth_kind(auth: &ResolvedAuth) -> &'static str {
    match auth {
        ResolvedAuth::None => "none",
        ResolvedAuth::ApiKey(_) => "api_key",
        ResolvedAuth::Custom(_) => "custom",
    }
}

fn emit_transport_trace(request: &Request, label: &str, data: Value) {
    if let Some(trace) = &request.transport_trace {
        trace.emit(label, data);
    }
}

fn json_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn request_body_shape_payload(body: &Value) -> Value {
    let mut map = Map::new();
    if let Some(input) = body.get("input").and_then(Value::as_array) {
        let items_json_bytes = serde_json::to_vec(input).map(|bytes| bytes.len()).ok();
        let mut reasoning_items = 0usize;
        let mut reasoning_encrypted_content_count = 0usize;
        let mut reasoning_encrypted_content_bytes = 0usize;
        for item in input {
            if item.get("type").and_then(Value::as_str) != Some("reasoning") {
                continue;
            }
            reasoning_items += 1;
            if let Some(encrypted) = item.get("encrypted_content").and_then(Value::as_str) {
                reasoning_encrypted_content_count += 1;
                reasoning_encrypted_content_bytes += encrypted.len();
            }
        }
        map.insert("items_len".to_string(), json!(input.len()));
        map.insert("items_json_bytes".to_string(), json!(items_json_bytes));
        map.insert("reasoning_items".to_string(), json!(reasoning_items));
        map.insert(
            "reasoning_encrypted_content_count".to_string(),
            json!(reasoning_encrypted_content_count),
        );
        map.insert(
            "reasoning_encrypted_content_bytes".to_string(),
            json!(reasoning_encrypted_content_bytes),
        );
    }
    Value::Object(map)
}

fn api_error_code(error: &ClientError) -> Option<&str> {
    match error {
        ClientError::Api { code, .. } => code.as_deref(),
        _ => None,
    }
}

fn is_context_length_exceeded(error: &ClientError) -> bool {
    match error {
        ClientError::Api { code, message, .. } => {
            code.as_deref() == Some("context_length_exceeded")
                || message.contains("context_length_exceeded")
        }
        _ => false,
    }
}

async fn response_with_timeout(
    future: impl std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
    timeout: Duration,
    phase: &'static str,
) -> Result<reqwest::Response, ClientError> {
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| ClientError::Timeout { phase, timeout })?
        .map_err(ClientError::Http)
}

impl<S: Scheme + Clone> Clone for HttpTransport<S> {
    fn clone(&self) -> Self {
        Self {
            http_client: self.http_client.clone(),
            scheme: self.scheme.clone(),
            model_id: self.model_id.clone(),
            base_url: self.base_url.clone(),
            auth: self.auth.clone(),
            capability: self.capability.clone(),
        }
    }
}

/// エラーレスポンスを `ClientError::Api` に変換する。
async fn classify_error_response(resp: reqwest::Response) -> ClientError {
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let text = resp.text().await.unwrap_or_default();
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
        let error = json.get("error").unwrap_or(&json);
        let code = error
            .get("code")
            .and_then(|v| v.as_str())
            .or_else(|| error.get("type").and_then(|v| v.as_str()))
            .map(String::from);
        let message = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or(&text)
            .to_string();
        ClientError::Api {
            status: Some(status),
            code,
            message,
            retry_after,
        }
    } else {
        ClientError::Api {
            status: Some(status),
            code: None,
            message: text,
            retry_after,
        }
    }
}

#[async_trait]
impl<S: Scheme + Clone + 'static> LlmClient for HttpTransport<S> {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    fn validate_config(&self, config: &RequestConfig) -> Vec<ConfigWarning> {
        self.scheme.validate_config(config)
    }

    async fn stream(&self, request: Request) -> Result<ResponseStream, ClientError> {
        let total_started = Instant::now();
        let path = self.scheme.path(&self.model_id);
        emit_transport_trace(
            &request,
            "transport_start",
            json!({
                "model": &self.model_id,
                "path": path,
                "auth_kind": auth_kind(&self.auth),
                "required_auth": format!("{:?}", self.scheme.required_auth()),
                "codex_backend": self.is_codex_backend(),
                "cache_key_present": request.cache_key.is_some(),
                "stream_open_timeout_ms": DEFAULT_STREAM_OPEN_TIMEOUT.as_millis() as u64,
            }),
        );

        let url = self.build_url();
        let headers_started = Instant::now();
        emit_transport_trace(
            &request,
            "transport_headers_start",
            json!({
                "auth_kind": auth_kind(&self.auth),
                "required_auth": format!("{:?}", self.scheme.required_auth()),
            }),
        );
        let mut headers = match self.build_headers().await {
            Ok(headers) => {
                emit_transport_trace(
                    &request,
                    "transport_headers_done",
                    json!({
                        "elapsed_ms": headers_started.elapsed().as_millis() as u64,
                        "headers_len": headers.len(),
                        "headers": request_header_diagnostics(&headers),
                    }),
                );
                headers
            }
            Err(error) => {
                emit_transport_trace(
                    &request,
                    "transport_headers_error",
                    json!({
                        "elapsed_ms": headers_started.elapsed().as_millis() as u64,
                        "error": error.to_string(),
                    }),
                );
                return Err(error);
            }
        };

        let stream_headers_started = Instant::now();
        if let Err(error) = self.apply_stream_headers(&mut headers, &request) {
            emit_transport_trace(
                &request,
                "transport_stream_headers_error",
                json!({
                    "elapsed_ms": stream_headers_started.elapsed().as_millis() as u64,
                    "error": error.to_string(),
                }),
            );
            return Err(error);
        }
        emit_transport_trace(
            &request,
            "transport_stream_headers_done",
            json!({
                "elapsed_ms": stream_headers_started.elapsed().as_millis() as u64,
                "headers_len": headers.len(),
                "headers": request_header_diagnostics(&headers),
            }),
        );

        let body_started = Instant::now();
        emit_transport_trace(&request, "transport_body_build_start", json!({}));
        let body = self
            .scheme
            .build_request_body(&self.model_id, &request, &self.capability);
        let body_shape = request_body_shape_payload(&body);
        emit_transport_trace(
            &request,
            "transport_body_build_done",
            json!({
                "elapsed_ms": body_started.elapsed().as_millis() as u64,
                "body_kind": json_value_kind(&body),
                "request_shape": body_shape.clone(),
            }),
        );

        let encode_started = Instant::now();
        let request_body = match self.encode_request_body(&body, &mut headers) {
            Ok(body) => body,
            Err(error) => {
                emit_transport_trace(
                    &request,
                    "transport_body_encode_error",
                    json!({
                        "elapsed_ms": encode_started.elapsed().as_millis() as u64,
                        "error": error.to_string(),
                    }),
                );
                return Err(error);
            }
        };
        let final_request_headers = request_header_diagnostics(&headers);
        let body_compression = request_body.encoding().to_string();
        emit_transport_trace(
            &request,
            "transport_body_encode_done",
            json!({
                "elapsed_ms": encode_started.elapsed().as_millis() as u64,
                "encoding": body_compression.as_str(),
                "body_compression": body_compression.as_str(),
                "raw_json_bytes": request_body.raw_json_bytes(),
                "wire_bytes": request_body.wire_bytes(),
                "request_shape": body_shape.clone(),
                "headers": final_request_headers.clone(),
            }),
        );

        let builder = self.http_client.post(&url).headers(headers);
        let builder = match request_body {
            RequestBody::Json(body) => builder.json(&body),
            RequestBody::CompressedJson { bytes, .. } => builder.body(bytes),
        };

        let send_started = Instant::now();
        emit_transport_trace(&request, "transport_http_send_start", json!({}));
        let response =
            match response_with_timeout(builder.send(), DEFAULT_STREAM_OPEN_TIMEOUT, "stream_open")
                .await
            {
                Ok(response) => {
                    let response_headers = response_header_diagnostics(response.headers());
                    emit_transport_trace(
                        &request,
                        "transport_http_headers_received",
                        json!({
                            "elapsed_ms": send_started.elapsed().as_millis() as u64,
                            "status": response.status().as_u16(),
                            "success": response.status().is_success(),
                            "headers": response_headers,
                        }),
                    );
                    response
                }
                Err(error) => {
                    emit_transport_trace(
                        &request,
                        "transport_http_send_error",
                        json!({
                            "elapsed_ms": send_started.elapsed().as_millis() as u64,
                            "error": error.to_string(),
                        }),
                    );
                    return Err(error);
                }
            };

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let retry_after_present = response.headers().get(RETRY_AFTER).is_some();
            let error = classify_error_response(response).await;
            let context_length_exceeded = is_context_length_exceeded(&error);
            emit_transport_trace(
                &request,
                "transport_http_status_error",
                json!({
                    "status": status,
                    "retry_after_present": retry_after_present,
                    "api_error_code": api_error_code(&error),
                    "context_length_exceeded": context_length_exceeded,
                    "provider_usage_absent": context_length_exceeded,
                    "request_shape": body_shape.clone(),
                    "request_headers": final_request_headers.clone(),
                    "body_compression": body_compression.as_str(),
                }),
            );
            return Err(error);
        }

        emit_transport_trace(
            &request,
            "transport_stream_ready",
            json!({
                "elapsed_ms": total_started.elapsed().as_millis() as u64,
            }),
        );

        let scheme = self.scheme.clone();
        let status = response.status().as_u16();
        let response_headers = response_header_diagnostics(response.headers());
        let transport_trace = request.transport_trace.clone();
        let byte_stream = response.bytes_stream().map_err(std::io::Error::other);
        let event_stream = byte_stream.eventsource();

        // scheme 固有のパース状態をストリーム単位で保持する
        let mut state = <S::State as Default>::default();

        let stream = event_stream
            .map(move |result| match result {
                Ok(frame) => match scheme.parse_sse(&frame.event, &frame.data, &mut state) {
                    Ok(events) => Ok(events),
                    Err(e) => Err(e),
                },
                Err(e) => {
                    let source = e.to_string();
                    let message = sse_error_context(status, &response_headers, &source);
                    if let Some(trace) = &transport_trace {
                        trace.emit(
                            "transport_sse_parse_error",
                            json!({
                                "status": status,
                                "headers": response_headers.clone(),
                                "error": source,
                            }),
                        );
                    }
                    Err(ClientError::Sse(message))
                }
            })
            .map(|res| {
                let s: Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>> = match res {
                    Ok(events) => Box::pin(futures::stream::iter(events.into_iter().map(Ok))),
                    Err(e) => Box::pin(futures::stream::once(async move { Err(e) })),
                };
                s
            })
            .flatten();

        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug)]
    struct TestAuthProvider {
        codex: bool,
    }

    #[async_trait]
    impl AuthProvider for TestAuthProvider {
        async fn headers(&self) -> Result<Vec<(HeaderName, HeaderValue)>, ClientError> {
            Ok(vec![
                (
                    HeaderName::from_static("authorization"),
                    HeaderValue::from_static("Bearer test-token"),
                ),
                (
                    HeaderName::from_static("chatgpt-account-id"),
                    HeaderValue::from_static("account-1"),
                ),
            ])
        }

        fn is_codex_backend(&self) -> bool {
            self.codex
        }
    }

    #[derive(Clone)]
    struct TestScheme;

    impl Scheme for TestScheme {
        type State = ();

        fn default_base_url(&self) -> &'static str {
            "https://example.test"
        }

        fn path(&self, _model_id: &str) -> String {
            "/responses".to_string()
        }

        fn required_auth(&self) -> AuthRequirement {
            AuthRequirement::Bearer
        }

        fn build_request_body(
            &self,
            model_id: &str,
            request: &Request,
            _capability: &ModelCapability,
        ) -> serde_json::Value {
            json!({
                "model": model_id,
                "input_len": request.items.len(),
                "prompt_cache_key": request.cache_key,
            })
        }

        fn parse_sse(
            &self,
            _event_type: &str,
            _data: &str,
            _state: &mut Self::State,
        ) -> Result<Vec<Event>, ClientError> {
            Ok(Vec::new())
        }

        fn default_capability(&self) -> ModelCapability {
            ModelCapability::minimal()
        }
    }

    fn transport(auth: ResolvedAuth) -> HttpTransport<TestScheme> {
        HttpTransport::new(
            TestScheme,
            "gpt-test",
            "https://example.test",
            auth,
            ModelCapability::minimal(),
        )
    }

    #[test]
    fn sse_error_context_includes_response_headers() {
        let headers = json!({
            "content_type": "application/octet-stream",
            "content_encoding": "gzip",
            "transfer_encoding": "chunked",
            "content_length": "123",
        });

        let message = sse_error_context(200, &headers, "stream did not contain valid UTF-8");

        assert!(message.contains("HTTP 200"));
        assert!(message.contains("stream did not contain valid UTF-8"));
        assert!(message.contains("content-type=application/octet-stream"));
        assert!(message.contains("content-encoding=gzip"));
        assert!(message.contains("transfer-encoding=chunked"));
        assert!(message.contains("content-length=123"));
    }

    #[test]
    fn response_header_diagnostics_redacts_to_safe_header_subset() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("identity"));
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));

        let diagnostics = response_header_diagnostics(&headers);

        assert_eq!(diagnostics["content_type"], "text/event-stream");
        assert_eq!(diagnostics["content_encoding"], "identity");
        assert!(diagnostics.get("authorization").is_none());
    }

    #[test]
    fn request_body_shape_counts_reasoning_encrypted_content() {
        let payload = request_body_shape_payload(&json!({
            "reasoning": { "summary": "auto" },
            "input": [
                { "type": "message", "role": "user", "content": [] },
                { "type": "reasoning", "encrypted_content": "abc", "summary": [] },
                { "type": "reasoning", "encrypted_content": "defgh", "summary": [] }
            ]
        }));
        assert_eq!(payload["items_len"], 3);
        assert_eq!(payload["reasoning_items"], 2);
        assert_eq!(payload["reasoning_encrypted_content_count"], 2);
        assert_eq!(payload["reasoning_encrypted_content_bytes"], 8);
        assert!(payload["items_json_bytes"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn response_timeout_returns_retryable_lifecycle_timeout() {
        let err = response_with_timeout(
            std::future::pending::<Result<reqwest::Response, reqwest::Error>>(),
            Duration::from_millis(5),
            "stream_open",
        )
        .await
        .unwrap_err();

        assert!(crate::llm_client::error::is_retryable(&err));
        assert!(matches!(
            err,
            ClientError::Timeout {
                phase: "stream_open",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn codex_backend_adds_conversation_headers_and_zstd_body() {
        let transport = transport(ResolvedAuth::Custom(Arc::new(TestAuthProvider {
            codex: true,
        })));
        let request = Request::new().user("hello").cache_key("segment-123");
        let mut headers = transport.build_headers().await.unwrap();
        transport
            .apply_stream_headers(&mut headers, &request)
            .unwrap();
        let body = transport.scheme.build_request_body(
            &transport.model_id,
            &request,
            &transport.capability,
        );
        let encoded = transport.encode_request_body(&body, &mut headers).unwrap();

        assert_eq!(headers.get(ACCEPT).unwrap(), "text/event-stream");
        assert_eq!(headers.get("session-id").unwrap(), "segment-123");
        assert_eq!(headers.get("thread-id").unwrap(), "segment-123");
        assert_eq!(headers.get("session_id").unwrap(), "segment-123");
        assert_eq!(headers.get("x-client-request-id").unwrap(), "segment-123");
        assert_eq!(headers.get(CONTENT_ENCODING).unwrap(), "zstd");

        let diagnostics = request_header_diagnostics(&headers);
        assert_eq!(diagnostics["content_type"], "application/json");
        assert_eq!(diagnostics["content_encoding"], "zstd");
        assert_eq!(diagnostics["accept"], "text/event-stream");
        assert!(diagnostics["session_id_present"].as_bool().unwrap());
        assert!(diagnostics["thread_id_present"].as_bool().unwrap());
        assert!(diagnostics["legacy_session_id_present"].as_bool().unwrap());
        assert!(
            diagnostics["x_client_request_id_present"]
                .as_bool()
                .unwrap()
        );
        assert!(diagnostics["chatgpt_account_id_present"].as_bool().unwrap());

        let RequestBody::CompressedJson {
            bytes: compressed,
            raw_json_bytes,
        } = encoded
        else {
            panic!("Codex backend request body must be zstd-compressed");
        };
        assert!(raw_json_bytes > 0);
        let decoded = zstd::stream::decode_all(std::io::Cursor::new(compressed)).unwrap();
        let decoded: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(decoded["prompt_cache_key"], "segment-123");
    }

    #[tokio::test]
    async fn non_codex_request_does_not_get_codex_only_headers_or_compression() {
        let transport = transport(ResolvedAuth::ApiKey("api-key".to_string()));
        let request = Request::new().user("hello").cache_key("segment-123");
        let mut headers = transport.build_headers().await.unwrap();
        transport
            .apply_stream_headers(&mut headers, &request)
            .unwrap();
        let body = transport.scheme.build_request_body(
            &transport.model_id,
            &request,
            &transport.capability,
        );
        let encoded = transport.encode_request_body(&body, &mut headers).unwrap();

        assert_eq!(headers.get(ACCEPT).unwrap(), "text/event-stream");
        assert!(headers.get("session-id").is_none());
        assert!(headers.get("thread-id").is_none());
        assert!(headers.get("session_id").is_none());
        assert!(headers.get("x-client-request-id").is_none());
        assert!(headers.get(CONTENT_ENCODING).is_none());

        let RequestBody::Json(decoded) = encoded else {
            panic!("non-Codex request body must remain normal JSON");
        };
        assert_eq!(decoded["prompt_cache_key"], "segment-123");
    }
}
