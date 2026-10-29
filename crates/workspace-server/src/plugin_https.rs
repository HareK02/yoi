//! Bounded, single-attempt HTTPS for an already admitted Plugin operation.
//!
//! The caller owns live authorization, secret lookup and durable effect admission.
//! In particular, an execution error is not evidence that an external effect did
//! not happen, and must never cause an automatic replay. No raw network error is
//! retained here. Debug output also omits request/response data and credentials.

use std::collections::BTreeSet;
use std::fmt;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use server_api::PluginHttpMethod;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::plugin_binding::{PluginConnectionBinding, PluginConnectionPolicyCeiling, path_within};

mod policy;
use policy::{PinnedResolver, check_addresses, safe_header};

const MAX_HEADERS: usize = 64;
const MAX_URL_BYTES: usize = 16 * 1024;
const MAX_QUERY_PAIRS: usize = 256;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpsRequest {
    pub method: PluginHttpMethod,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout_ms: u32,
    pub effect_key: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpsResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl fmt::Debug for HttpsRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpsRequest").finish_non_exhaustive()
    }
}

impl fmt::Debug for HttpsResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpsResponse")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// Closed, safe categories: no reqwest, URL, DNS, TLS or header parsing source
/// error is exposed. These categories do not authorize retry after execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("invalid HTTPS binding")]
    InvalidBinding,
    #[error("invalid HTTPS request")]
    InvalidRequest,
    #[error("HTTPS request denied")]
    Denied,
    #[error("HTTPS header limit exceeded")]
    HeaderLimit,
    #[error("HTTPS request limit exceeded")]
    RequestLimit,
    #[error("HTTPS response limit exceeded")]
    ResponseLimit,
    #[error("HTTPS name resolution failed")]
    Dns,
    #[error("HTTPS transport unavailable")]
    Unavailable,
    #[error("HTTPS deadline exceeded")]
    DeadlineExceeded,
    #[error("HTTPS request cancelled")]
    Cancelled,
    #[error("HTTPS credential unavailable or invalid")]
    InvalidSecret,
    #[error("unsupported HTTPS response encoding")]
    UnsupportedEncoding,
}

/// Private fields prevent changing the checked request or swapping its binding.
/// The caller can hash/serialize `normalized()` for its durable fingerprint;
/// secret values are never part of this representation. Receipt fingerprinting
/// must exclude timeout and replace the body with its hash as required by API.md.
#[derive(Clone)]
pub struct PreparedRequest {
    request: HttpsRequest,
    binding_digest: String,
    url: Url,
}

impl PreparedRequest {
    pub fn normalized(&self) -> &HttpsRequest {
        &self.request
    }
}

impl fmt::Debug for PreparedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedRequest").finish_non_exhaustive()
    }
}

/// Additional operator-owned exclusions, not Workspace-granted exceptions.
/// Operators must include all public control-plane IPs/names here. IP exclusions
/// apply to every port, including literal-IP origins and every DNS candidate.
#[derive(Debug, Clone)]
pub struct TransportPolicy {
    pub denied_addresses: BTreeSet<IpAddr>,
    pub denied_hosts: BTreeSet<String>,
}

pub struct HttpsTransport {
    policy: TransportPolicy,
    #[cfg(test)]
    test_overrides: Option<test_support::TestOverrides>,
}

pub fn prepare(
    binding: &PluginConnectionBinding,
    mut request: HttpsRequest,
) -> Result<PreparedRequest, TransportError> {
    binding
        .validate(&PluginConnectionPolicyCeiling::default())
        .map_err(|_| TransportError::InvalidBinding)?;
    if !binding.methods.contains(&request.method)
        || !path_within("/", &request.path)
        || !binding.allows_path(&request.path)
    {
        return Err(TransportError::Denied);
    }
    if request.timeout_ms == 0
        || request.effect_key.is_empty()
        || request.effect_key.len() > 128
        || request.effect_key.chars().any(char::is_control)
    {
        return Err(TransportError::InvalidRequest);
    }
    if request.body.len() > binding.limits.max_request_bytes as usize
        || request.query.len() > MAX_QUERY_PAIRS
    {
        return Err(TransportError::RequestLimit);
    }
    request.timeout_ms = request.timeout_ms.min(binding.limits.timeout_ms);
    if request.headers.len() > MAX_HEADERS {
        return Err(TransportError::HeaderLimit);
    }
    let mut header_bytes = 0usize;
    for (name, value) in &mut request.headers {
        let parsed =
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| TransportError::InvalidRequest)?;
        *name = parsed.as_str().to_owned();
        if !safe_header(name) || !binding.request_headers.contains(name) {
            return Err(TransportError::Denied);
        }
        // Exclude folding, control bytes and non-ASCII ambiguities even if the
        // HTTP parser would accept them. Header values are never trimmed.
        if !value.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err(TransportError::InvalidRequest);
        }
        HeaderValue::from_str(value).map_err(|_| TransportError::InvalidRequest)?;
        header_bytes = header_bytes.saturating_add(name.len() + value.len() + 4);
    }
    if header_bytes > binding.limits.max_header_bytes as usize {
        return Err(TransportError::HeaderLimit);
    }
    // Bound raw query input before percent encoding (whose expansion is at most
    // threefold), then bound the actual request target after encoding as well.
    let query_bytes = request
        .query
        .iter()
        .try_fold(0usize, |sum, (k, v)| {
            sum.checked_add(k.len())?
                .checked_add(v.len())?
                .checked_add(2)
        })
        .ok_or(TransportError::RequestLimit)?;
    if query_bytes > MAX_URL_BYTES {
        return Err(TransportError::RequestLimit);
    }
    let mut url = Url::parse(&format!("{}{}", binding.origin, request.path))
        .map_err(|_| TransportError::InvalidRequest)?;
    if url.path() != request.path {
        return Err(TransportError::Denied);
    }
    if !request.query.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(request.query.iter().map(|(k, v)| (k, v)));
    }
    if url.as_str().len() > MAX_URL_BYTES {
        return Err(TransportError::RequestLimit);
    }
    Ok(PreparedRequest {
        request,
        binding_digest: binding.digest(),
        url,
    })
}

impl HttpsTransport {
    pub fn new(policy: TransportPolicy) -> Self {
        Self {
            policy,
            #[cfg(test)]
            test_overrides: None,
        }
    }

    /// `secret` is the complete Authorization field value (including its scheme),
    /// not a secret reference. Its presence must agree with binding.secret_ref.
    /// A fresh client and a pinned resolver are used for every single attempt;
    /// neither a cached connection nor a second OS resolution can bypass checks.
    /// The caller must cancel this token at the enclosing invocation deadline.
    pub async fn execute(
        &self,
        binding: &PluginConnectionBinding,
        prepared: PreparedRequest,
        secret: Option<&str>,
        cancellation: &CancellationToken,
    ) -> Result<HttpsResponse, TransportError> {
        self.execute_with_guard(binding, prepared, secret, cancellation, || async { Ok(()) })
            .await
    }

    /// Run the caller's fresh authority check only after DNS, address admission,
    /// credential/header checks and complete client/request construction. Denial
    /// is returned unchanged and no connection is opened. The guard shares the
    /// transport deadline and cancellation; there is no await between a successful
    /// guard and dispatch. It cannot make authorization atomic with remote I/O.
    pub async fn execute_with_guard<G, F>(
        &self,
        binding: &PluginConnectionBinding,
        prepared: PreparedRequest,
        secret: Option<&str>,
        cancellation: &CancellationToken,
        guard: G,
    ) -> Result<HttpsResponse, TransportError>
    where
        G: FnOnce() -> F + Send,
        F: std::future::Future<Output = Result<(), TransportError>> + Send,
    {
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(prepared.request.timeout_ms.into());
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(TransportError::Cancelled),
            result = tokio::time::timeout_at(deadline, self.execute_inner(binding, prepared, secret, cancellation, deadline, guard)) => {
                result.unwrap_or(Err(TransportError::DeadlineExceeded))
            }
        }
    }

    async fn execute_inner<G, F>(
        &self,
        binding: &PluginConnectionBinding,
        prepared: PreparedRequest,
        secret: Option<&str>,
        cancellation: &CancellationToken,
        deadline: tokio::time::Instant,
        guard: G,
    ) -> Result<HttpsResponse, TransportError>
    where
        G: FnOnce() -> F + Send,
        F: std::future::Future<Output = Result<(), TransportError>> + Send,
    {
        // Recheck against the supplied current binding, not only the preparation
        // snapshot. A stale prepared policy cannot silently gain new authority.
        if prepared.binding_digest != binding.digest() {
            return Err(TransportError::Denied);
        }
        let prepared = prepare(binding, prepared.request)?;
        let host = prepared
            .url
            .host_str()
            .ok_or(TransportError::InvalidBinding)?;
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if self
            .policy
            .denied_hosts
            .iter()
            .any(|name| name.eq_ignore_ascii_case(host))
        {
            return Err(TransportError::Denied);
        }
        let port = prepared
            .url
            .port_or_known_default()
            .ok_or(TransportError::InvalidBinding)?;
        let addresses = self.resolve(host, port).await?;
        check_addresses(&addresses, &self.policy)?;

        let mut headers = HeaderMap::new();
        for (name, value) in &prepared.request.headers {
            headers.append(
                HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| TransportError::InvalidRequest)?,
                HeaderValue::from_str(value).map_err(|_| TransportError::InvalidRequest)?,
            );
        }
        match (binding.secret_ref.is_empty(), secret) {
            (true, None) => {}
            (false, Some(value)) if !value.is_empty() => {
                if value.len() > binding.limits.max_header_bytes as usize {
                    return Err(TransportError::HeaderLimit);
                }
                if !value.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
                    return Err(TransportError::InvalidSecret);
                }
                let mut value =
                    HeaderValue::from_str(value).map_err(|_| TransportError::InvalidSecret)?;
                value.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, value);
            }
            _ => return Err(TransportError::InvalidSecret),
        }
        headers.insert(
            reqwest::header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip"),
        );
        headers.insert(
            reqwest::header::CONNECTION,
            HeaderValue::from_static("close"),
        );
        headers
            .entry(reqwest::header::ACCEPT)
            .or_insert(HeaderValue::from_static("*/*"));
        // Include generated fields and the credential in the wire header budget.
        let authority = prepared.url[url::Position::BeforeHost..url::Position::AfterPort].len();
        check_header_budget(&headers, authority + "host: \r\n".len() + 32, binding)?;

        #[cfg(test)]
        let addresses = self
            .test_overrides
            .as_ref()
            .and_then(|o| o.connect_to)
            .map(|address| vec![address])
            .unwrap_or(addresses);
        let resolver = PinnedResolver::new(host.to_owned(), addresses);
        let builder = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .http1_only()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .tls_backend_native()
            .tls_version_min(reqwest::tls::Version::TLS_1_2)
            .tls_sni(true)
            .dns_resolver(resolver)
            .pool_max_idle_per_host(0);
        #[cfg(test)]
        let builder = match self.test_overrides.as_ref().and_then(|o| o.root.clone()) {
            Some(root) => builder.tls_certs_only([root]),
            None => builder,
        };
        let client = builder.build().map_err(|_| TransportError::Unavailable)?;
        let request = client
            .request(method(&prepared.request.method), prepared.url)
            .headers(headers)
            .body(prepared.request.body)
            .build()
            .map_err(|_| TransportError::InvalidRequest)?;
        guard().await?;
        if cancellation.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(TransportError::DeadlineExceeded);
        }
        let mut response = client.execute(request).await.map_err(network_error)?;
        check_header_budget(response.headers(), 0, binding)?;
        let status = response.status().as_u16();
        let encoding = response_encoding(response.headers())?;
        let limit = binding.limits.max_response_bytes as usize;
        if response.content_length().is_some_and(|n| n > limit as u64) {
            return Err(TransportError::ResponseLimit);
        }
        let connection_headers = connection_headers(response.headers())?;
        let mut returned_headers = Vec::new();
        for (name, value) in response.headers() {
            if safe_header(name.as_str())
                && !connection_headers.contains(name.as_str())
                && binding.response_headers.iter().any(|h| h == name.as_str())
                && !(encoding == Encoding::Gzip && name == reqwest::header::CONTENT_ENCODING)
            {
                let value = value.to_str().map_err(|_| TransportError::Unavailable)?;
                returned_headers.push((name.as_str().to_owned(), value.to_owned()));
            }
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network_error)? {
            if chunk.len() > limit.saturating_sub(body.len()) {
                return Err(TransportError::ResponseLimit);
            }
            body.extend_from_slice(&chunk);
        }
        if encoding == Encoding::Gzip {
            body = decode_gzip(&body, limit).await?;
        }
        Ok(HttpsResponse {
            status,
            headers: returned_headers,
            body,
        })
    }

    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, TransportError> {
        #[cfg(test)]
        if let Some(overrides) = &self.test_overrides {
            return overrides.resolve().await;
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![SocketAddr::new(ip, port)]);
        }
        tokio::net::lookup_host((host, port))
            .await
            .map(|addresses| addresses.collect())
            .map_err(|_| TransportError::Dns)
    }
}

fn check_header_budget(
    headers: &HeaderMap,
    generated_bytes: usize,
    binding: &PluginConnectionBinding,
) -> Result<(), TransportError> {
    let bytes = headers.iter().fold(generated_bytes, |sum, (name, value)| {
        sum.saturating_add(name.as_str().len() + value.as_bytes().len() + 4)
    });
    // Generated Host and Content-Length consume two additional fields.
    let generated_count = if generated_bytes == 0 { 0 } else { 2 };
    if headers.len() + generated_count > MAX_HEADERS
        || bytes > binding.limits.max_header_bytes as usize
    {
        return Err(TransportError::HeaderLimit);
    }
    Ok(())
}

fn connection_headers(headers: &HeaderMap) -> Result<BTreeSet<String>, TransportError> {
    let mut nominated = BTreeSet::new();
    for value in headers.get_all(reqwest::header::CONNECTION) {
        let value = value.to_str().map_err(|_| TransportError::Unavailable)?;
        for name in value.split(',') {
            let name = HeaderName::from_bytes(name.trim().as_bytes())
                .map_err(|_| TransportError::Unavailable)?;
            nominated.insert(name.as_str().to_owned());
        }
    }
    Ok(nominated)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Identity,
    Gzip,
}

fn response_encoding(headers: &HeaderMap) -> Result<Encoding, TransportError> {
    let mut values = headers.get_all(reqwest::header::CONTENT_ENCODING).iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(TransportError::UnsupportedEncoding);
    }
    match first {
        None => Ok(Encoding::Identity),
        Some(value) if value.as_bytes().eq_ignore_ascii_case(b"identity") => Ok(Encoding::Identity),
        Some(value) if value.as_bytes().eq_ignore_ascii_case(b"gzip") => Ok(Encoding::Gzip),
        _ => Err(TransportError::UnsupportedEncoding),
    }
}

async fn decode_gzip(bytes: &[u8], limit: usize) -> Result<Vec<u8>, TransportError> {
    let mut decoder = flate2::read::MultiGzDecoder::new(bytes);
    let mut output = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let length = decoder
            .read(&mut chunk)
            .map_err(|_| TransportError::Unavailable)?;
        if length == 0 {
            break;
        }
        if length > limit.saturating_sub(output.len()) {
            return Err(TransportError::ResponseLimit);
        }
        output.extend_from_slice(&chunk[..length]);
        // Keep cancellation and the enclosing wall deadline observable even for
        // concatenated compressed members and expansion-heavy payloads.
        tokio::task::yield_now().await;
    }
    Ok(output)
}

fn network_error(error: reqwest::Error) -> TransportError {
    if error.is_timeout() {
        TransportError::DeadlineExceeded
    } else {
        TransportError::Unavailable
    }
}

fn method(method: &PluginHttpMethod) -> reqwest::Method {
    match method {
        PluginHttpMethod::Get => reqwest::Method::GET,
        PluginHttpMethod::Head => reqwest::Method::HEAD,
        PluginHttpMethod::Post => reqwest::Method::POST,
        PluginHttpMethod::Put => reqwest::Method::PUT,
        PluginHttpMethod::Patch => reqwest::Method::PATCH,
        PluginHttpMethod::Delete => reqwest::Method::DELETE,
        PluginHttpMethod::Options => reqwest::Method::OPTIONS,
    }
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
