//! Static Workspace HTTPS policy, not network admission. Runtime must recheck live
//! binding state, resolve/check every DNS address, pin the connection, verify TLS,
//! inject credentials, and enforce these bounds (including decompressed bytes).
//! Plans retain the binding identity and digest, never this destination policy.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};
use server_api::PluginHttpMethod;
use sha2::{Digest, Sha256};
use url::{Host, Url};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PluginNetworkPolicy {
    /// DNS names are provisional until all resolved addresses pass live checks.
    /// Private-network exceptions are deliberately not supported by this policy.
    #[default]
    PublicInternet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConnectionLimits {
    pub timeout_ms: u32,
    pub max_request_bytes: u32,
    pub max_response_bytes: u32,
    pub max_header_bytes: u32,
}

/// Trusted server operational ceiling, supplied separately from Workspace input.
/// Deserializing a binding cannot set or expand this ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginConnectionPolicyCeiling {
    pub limits: PluginConnectionLimits,
}

impl Default for PluginConnectionPolicyCeiling {
    fn default() -> Self {
        Self {
            limits: PluginConnectionLimits {
                timeout_ms: 15_000,
                max_request_bytes: 1024 * 1024,
                max_response_bytes: 1024 * 1024,
                max_header_bytes: 16 * 1024,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginConnectionBinding {
    /// Canonical origin only: no trailing slash, path, query, credentials or fragment.
    pub origin: String,
    #[serde(default)]
    pub network: PluginNetworkPolicy,
    pub methods: Vec<PluginHttpMethod>,
    pub path_prefixes: Vec<String>,
    #[serde(default)]
    pub request_headers: Vec<String>,
    #[serde(default)]
    pub response_headers: Vec<String>,
    /// Empty means no credential. Otherwise an opaque Workspace-local secret ID;
    /// never a value, token, provider URL, or filesystem reference.
    #[serde(default)]
    pub secret_ref: String,
    pub limits: PluginConnectionLimits,
}

/// Safe diagnostic categories contain no caller-supplied URLs, headers or secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PluginBindingError {
    #[error("invalid plugin binding identity")]
    InvalidIdentity,
    #[error("plugin connection requires an exact canonical HTTPS origin")]
    InvalidOrigin,
    #[error("plugin connection destination is not public internet")]
    NonPublicDestination,
    #[error("plugin connection methods must be nonempty and unique")]
    InvalidMethods,
    #[error("plugin connection paths must be nonempty, unique canonical segment prefixes")]
    InvalidPaths,
    #[error("plugin connection header allowlist is invalid or contains a reserved header")]
    InvalidHeaders,
    #[error("plugin connection secret must be an opaque reference")]
    InvalidSecretReference,
    #[error("plugin connection limits must be positive and within the server ceiling")]
    InvalidLimits,
    #[error("too many plugin bindings")]
    TooManyBindings,
}

impl PluginConnectionBinding {
    pub fn validate(
        &self,
        ceiling: &PluginConnectionPolicyCeiling,
    ) -> Result<(), PluginBindingError> {
        validate_origin(&self.origin)?;
        if self.methods.is_empty() || self.methods.len() > 7 {
            return Err(PluginBindingError::InvalidMethods);
        }
        let mut methods = BTreeSet::new();
        if self
            .methods
            .iter()
            .any(|method| !methods.insert(method_name(method)))
        {
            return Err(PluginBindingError::InvalidMethods);
        }
        if self.path_prefixes.is_empty()
            || self.path_prefixes.len() > 64
            || !unique_strings(&self.path_prefixes)
            || self.path_prefixes.iter().any(|path| !canonical_path(path))
        {
            return Err(PluginBindingError::InvalidPaths);
        }
        for headers in [&self.request_headers, &self.response_headers] {
            if headers.len() > 64
                || !unique_strings(headers)
                || headers.iter().any(|header| !allowed_header(header))
            {
                return Err(PluginBindingError::InvalidHeaders);
            }
        }
        if !self.secret_ref.is_empty() && !opaque_reference(&self.secret_ref) {
            return Err(PluginBindingError::InvalidSecretReference);
        }
        for (value, maximum) in [
            (self.limits.timeout_ms, ceiling.limits.timeout_ms),
            (
                self.limits.max_request_bytes,
                ceiling.limits.max_request_bytes,
            ),
            (
                self.limits.max_response_bytes,
                ceiling.limits.max_response_bytes,
            ),
            (
                self.limits.max_header_bytes,
                ceiling.limits.max_header_bytes,
            ),
        ] {
            if value == 0 || maximum == 0 || value > maximum {
                return Err(PluginBindingError::InvalidLimits);
            }
        }
        Ok(())
    }

    /// Versioned SHA-256 of policy content. Allowlists have set semantics, so their
    /// order does not affect identity. Call `validate` before accepting a digest.
    /// Binding map identity is separate and must also be pinned by the caller.
    pub fn digest(&self) -> String {
        let mut canonical = self.clone();
        canonical.methods.sort_by_key(method_name);
        canonical.path_prefixes.sort();
        canonical.request_headers.sort();
        canonical.response_headers.sort();
        let bytes = serde_json::to_vec(&canonical).expect("binding contains only JSON primitives");
        let mut hash = Sha256::new();
        hash.update(b"yoi-plugin-connection-policy-v1\0");
        hash.update(bytes);
        let hex: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("sha256:{hex}")
    }

    /// Static segment-boundary matching only; this is not runtime authorization.
    /// Conservative paths exclude percent encodings to avoid decoder disagreement.
    pub fn allows_path(&self, path: &str) -> bool {
        self.path_prefixes
            .iter()
            .any(|prefix| path_within(prefix, path))
    }
}

/// Validates the Workspace-owned map without copying any destination into a plan.
/// The map key is the binding identity; pin that key and `binding.digest()` only.
pub fn validate_bindings(
    bindings: &BTreeMap<String, PluginConnectionBinding>,
    ceiling: &PluginConnectionPolicyCeiling,
) -> Result<(), PluginBindingError> {
    if bindings.len() > 256 {
        return Err(PluginBindingError::TooManyBindings);
    }
    for (identity, binding) in bindings {
        if !opaque_reference(identity) {
            return Err(PluginBindingError::InvalidIdentity);
        }
        binding.validate(ceiling)?;
    }
    Ok(())
}

/// Conservative canonical path containment on segment boundaries, not a raw
/// string prefix. Runtime must use the same canonical path for checking/sending.
pub fn path_within(prefix: &str, path: &str) -> bool {
    canonical_path(prefix)
        && canonical_path(path)
        && (prefix == "/"
            || path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/')))
}

fn method_name(method: &PluginHttpMethod) -> &'static str {
    match method {
        PluginHttpMethod::Get => "GET",
        PluginHttpMethod::Head => "HEAD",
        PluginHttpMethod::Post => "POST",
        PluginHttpMethod::Put => "PUT",
        PluginHttpMethod::Patch => "PATCH",
        PluginHttpMethod::Delete => "DELETE",
        PluginHttpMethod::Options => "OPTIONS",
    }
}

fn opaque_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn unique_strings(values: &[String]) -> bool {
    values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

fn canonical_path(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    path.len() <= 2048
        && path.starts_with('/')
        && path.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'/' | b'-'
                        | b'_'
                        | b'.'
                        | b'~'
                        | b'!'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b'+'
                        | b','
                        | b';'
                        | b'='
                        | b':'
                        | b'@'
                )
        })
        && path[1..]
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

fn allowed_header(header: &str) -> bool {
    !header.is_empty()
        && header.len() <= 128
        && header
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !header.starts_with("proxy-")
        && !header.starts_with("sec-")
        && !matches!(
            header,
            "authorization"
                | "www-authenticate"
                | "cookie"
                | "set-cookie"
                | "host"
                | "connection"
                | "keep-alive"
                | "transfer-encoding"
                | "te"
                | "trailer"
                | "upgrade"
                | "content-length"
                | "expect"
                | "forwarded"
                | "via"
                | "x-forwarded-for"
                | "x-forwarded-host"
                | "x-forwarded-proto"
                | "x-api-key"
                | "api-key"
        )
}

fn validate_origin(origin: &str) -> Result<(), PluginBindingError> {
    if origin.len() > 512 || !origin.is_ascii() {
        return Err(PluginBindingError::InvalidOrigin);
    }
    let url = Url::parse(origin).map_err(|_| PluginBindingError::InvalidOrigin)?;
    if url.scheme() != "https" || url.origin().ascii_serialization() != origin {
        return Err(PluginBindingError::InvalidOrigin);
    }
    let public = match url.host().ok_or(PluginBindingError::InvalidOrigin)? {
        Host::Domain(host) => {
            let labels: Vec<_> = host.split('.').collect();
            labels.len() >= 2
                && host.len() <= 253
                && labels.iter().all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label.as_bytes()[0].is_ascii_alphanumeric()
                        && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                        && label
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                })
                && ![
                    "localhost",
                    "local",
                    "internal",
                    "home",
                    "lan",
                    "test",
                    "invalid",
                    "onion",
                    "arpa",
                ]
                .iter()
                .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
        }
        Host::Ipv4(ip) => public_ipv4(ip),
        Host::Ipv6(ip) => public_ipv6(ip),
    };
    if !public {
        return Err(PluginBindingError::NonPublicDestination);
    }
    Ok(())
}

fn public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    // Conservative special-purpose exclusion; no Workspace-configurable bypass.
    !matches!(a, 0 | 10 | 127 | 224..=255)
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..=31).contains(&b))
        && !(a == 192 && (b == 0 || b == 168 || (b == 88 && c == 99)))
        && !(a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        && !(a == 203 && b == 0 && c == 113)
}

fn public_ipv6(ip: Ipv6Addr) -> bool {
    let [a, b, ..] = ip.segments();
    // Only current global-unicast space; reject transition/special-purpose and
    // documentation ranges as well as mapped IPv4, local, multicast and ULA.
    (0x2000..=0x3fff).contains(&a)
        && !(a == 0x2001 && (b <= 0x01ff || b == 0x0db8))
        && a != 0x2002
        && !(a == 0x3fff && b <= 0x0fff)
}

#[cfg(test)]
mod tests;
