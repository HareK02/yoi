//! Authentication requirements for `Scheme` implementations and the transport layer,
//! plus dynamic authentication providers.
//!
//! `AuthRequirement` describes the authentication a scheme expects at runtime.
//! The caller is responsible for building [`super::transport::ResolvedAuth`]
//! from configuration files, environment variables, or other sources.
//!
//! Authentication whose tokens may change per request implements [`AuthProvider`]
//! and is passed to the transport via [`super::transport::ResolvedAuth::Custom`].

use async_trait::async_trait;
use reqwest::header::{HeaderName, HeaderValue};

use super::error::ClientError;

/// Authentication requirements returned by `Scheme::required_auth()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRequirement {
    /// No authentication (e.g. Ollama).
    None,
    /// `Authorization: Bearer <token>` header (the token acts as an API key).
    Bearer,
    /// `x-api-key: <token>` header (Anthropic format).
    XApiKey,
    /// Query parameter `?<name>=<token>` (Gemini format).
    QueryParam { name: &'static str },
    /// Combined headers (resolved by the caller through [`AuthProvider`]).
    Custom,
}

/// Provider that builds authentication headers dynamically for each request.
///
/// Use this when access tokens are refreshed or multiple headers must be injected
/// together. The implementation belongs to the caller; agen only knows the trait.
///
/// Returned headers are inserted directly into `HeaderMap`. The scheme's default
/// authentication headers, including `Authorization`, are not sent; the
/// implementation must set them if needed.
#[async_trait]
pub trait AuthProvider: Send + Sync + std::fmt::Debug {
    /// Return authentication headers for one request, refreshing internally if needed.
    async fn headers(&self) -> Result<Vec<(HeaderName, HeaderValue)>, ClientError>;
}
