use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

const PUBLIC_KEY_PREFIX: &str = "yoi-ed25519-pub:v1:";
const PRIVATE_KEY_PREFIX: &str = "yoi-ed25519-pkcs8:v1:";
const TOKEN_PREFIX: &str = "yoi-cap-v1";
const SIGNING_INPUT_PREFIX: &str = "yoi-cap-v1.";

#[derive(Debug, thiserror::Error)]
pub enum RuntimeAuthError {
    #[error("invalid Ed25519 public key format")]
    InvalidPublicKeyFormat,
    #[error("invalid Ed25519 private key format")]
    InvalidPrivateKeyFormat,
    #[error("invalid base64url key material: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
    #[error("invalid Ed25519 private key")]
    InvalidPrivateKey,
    #[error("failed to generate Ed25519 keypair")]
    KeyGeneration,
    #[error("failed to generate random token id")]
    Random,
    #[error("invalid capability token format")]
    InvalidTokenFormat,
    #[error("malformed capability token claims: {0}")]
    MalformedClaims(#[from] serde_json::Error),
    #[error("unknown token issuer `{0}`")]
    UnknownIssuer(String),
    #[error("invalid token signature")]
    InvalidSignature,
    #[error("token audience `{actual}` does not match runtime `{expected}`")]
    WrongAudience { expected: String, actual: String },
    #[error("capability token is expired")]
    Expired,
    #[error("capability token is missing required permission `{0}`")]
    MissingPermission(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeIdentityMaterial {
    pub identity_id: String,
    pub public_key: String,
    pub private_key: String,
}

impl RuntimeIdentityMaterial {
    pub fn generate(identity_id: impl Into<String>) -> Result<Self, RuntimeAuthError> {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|_| RuntimeAuthError::KeyGeneration)?;
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .map_err(|_| RuntimeAuthError::InvalidPrivateKey)?;
        Ok(Self {
            identity_id: identity_id.into(),
            public_key: encode_public_key(pair.public_key().as_ref()),
            private_key: encode_private_key(pkcs8.as_ref()),
        })
    }

    pub fn signing_key(&self) -> Result<Ed25519KeyPair, RuntimeAuthError> {
        let private = decode_private_key(&self.private_key)?;
        Ed25519KeyPair::from_pkcs8(&private).map_err(|_| RuntimeAuthError::InvalidPrivateKey)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedServerKey {
    pub server_id: String,
    pub public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpAuthConfig {
    pub runtime_id: String,
    #[serde(default)]
    pub trusted_servers: Vec<TrustedServerKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeAuthContext {
    pub server_id: String,
    pub workspace_id: String,
    pub permissions: Vec<String>,
    pub token_id: String,
    pub expires_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityClaims {
    pub iss: String,
    pub aud: String,
    pub workspace_id: String,
    pub permissions: Vec<String>,
    pub exp: u64,
    pub jti: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityTokenSigner {
    server_id: String,
    private_key: String,
}

impl CapabilityTokenSigner {
    pub fn new(server_id: impl Into<String>, private_key: impl Into<String>) -> Self {
        Self {
            server_id: server_id.into(),
            private_key: private_key.into(),
        }
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    pub fn sign(&self, claims: &CapabilityClaims) -> Result<String, RuntimeAuthError> {
        if claims.iss != self.server_id {
            return Err(RuntimeAuthError::UnknownIssuer(claims.iss.clone()));
        }
        let private = decode_private_key(&self.private_key)?;
        let pair = Ed25519KeyPair::from_pkcs8(&private)
            .map_err(|_| RuntimeAuthError::InvalidPrivateKey)?;
        let payload = serde_json::to_vec(claims)?;
        let payload = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{SIGNING_INPUT_PREFIX}{payload}");
        let signature = pair.sign(signing_input.as_bytes());
        Ok(format!("{TOKEN_PREFIX}.{payload}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref())))
    }
}

pub fn capability_claims(
    server_id: impl Into<String>,
    runtime_id: impl Into<String>,
    workspace_id: impl Into<String>,
    permissions: Vec<String>,
    ttl_seconds: u64,
) -> Result<CapabilityClaims, RuntimeAuthError> {
    let exp = unix_now_seconds().saturating_add(ttl_seconds);
    Ok(CapabilityClaims {
        iss: server_id.into(),
        aud: runtime_id.into(),
        workspace_id: workspace_id.into(),
        permissions,
        exp,
        jti: new_token_id()?,
    })
}

pub fn verify_capability_token(
    config: &RuntimeHttpAuthConfig,
    token: &str,
    required_permission: Option<&str>,
    now_seconds: u64,
) -> Result<RuntimeAuthContext, RuntimeAuthError> {
    let (payload, signature) = split_token(token)?;
    let claims_json = URL_SAFE_NO_PAD.decode(payload)?;
    let claims: CapabilityClaims = serde_json::from_slice(&claims_json)?;
    let Some(server) = config
        .trusted_servers
        .iter()
        .find(|server| server.server_id == claims.iss)
    else {
        return Err(RuntimeAuthError::UnknownIssuer(claims.iss));
    };
    let public_key = decode_public_key(&server.public_key)?;
    let signing_input = format!("{SIGNING_INPUT_PREFIX}{payload}");
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| RuntimeAuthError::InvalidSignature)?;

    if claims.aud != config.runtime_id {
        return Err(RuntimeAuthError::WrongAudience {
            expected: config.runtime_id.clone(),
            actual: claims.aud,
        });
    }
    if claims.exp < now_seconds {
        return Err(RuntimeAuthError::Expired);
    }
    if let Some(required) = required_permission {
        if !claims.permissions.iter().any(|permission| permission == required) {
            return Err(RuntimeAuthError::MissingPermission(required.to_string()));
        }
    }
    Ok(RuntimeAuthContext {
        server_id: claims.iss,
        workspace_id: claims.workspace_id,
        permissions: claims.permissions,
        token_id: claims.jti,
        expires_at: claims.exp,
    })
}

fn split_token(token: &str) -> Result<(&str, Vec<u8>), RuntimeAuthError> {
    let mut parts = token.split('.');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(prefix), Some(payload), Some(signature), None) if prefix == TOKEN_PREFIX => {
            Ok((payload, URL_SAFE_NO_PAD.decode(signature)?))
        }
        _ => Err(RuntimeAuthError::InvalidTokenFormat),
    }
}

pub fn encode_public_key(bytes: &[u8]) -> String {
    format!("{PUBLIC_KEY_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

pub fn decode_public_key(value: &str) -> Result<Vec<u8>, RuntimeAuthError> {
    let Some(encoded) = value.strip_prefix(PUBLIC_KEY_PREFIX) else {
        return Err(RuntimeAuthError::InvalidPublicKeyFormat);
    };
    let bytes = URL_SAFE_NO_PAD.decode(encoded)?;
    if bytes.len() != 32 {
        return Err(RuntimeAuthError::InvalidPublicKeyFormat);
    }
    Ok(bytes)
}

pub fn encode_private_key(bytes: &[u8]) -> String {
    format!("{PRIVATE_KEY_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

pub fn decode_private_key(value: &str) -> Result<Vec<u8>, RuntimeAuthError> {
    let Some(encoded) = value.strip_prefix(PRIVATE_KEY_PREFIX) else {
        return Err(RuntimeAuthError::InvalidPrivateKeyFormat);
    };
    Ok(URL_SAFE_NO_PAD.decode(encoded)?)
}

pub fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn new_token_id() -> Result<String, RuntimeAuthError> {
    let rng = SystemRandom::new();
    let mut bytes = [0_u8; 16];
    rng.fill(&mut bytes).map_err(|_| RuntimeAuthError::Random)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

impl fmt::Display for RuntimeAuthContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "server={} workspace={} permissions={} exp={}",
            self.server_id,
            self.workspace_id,
            self.permissions.join(","),
            self.expires_at
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_token_verifies_signature_audience_expiry_and_permission() {
        let server = RuntimeIdentityMaterial::generate("server-main").unwrap();
        let signer = CapabilityTokenSigner::new(&server.identity_id, &server.private_key);
        let claims = CapabilityClaims {
            iss: "server-main".to_string(),
            aud: "runtime-main".to_string(),
            workspace_id: "workspace-a".to_string(),
            permissions: vec!["workers:list".to_string()],
            exp: 100,
            jti: "token-1".to_string(),
        };
        let token = signer.sign(&claims).unwrap();
        let auth = RuntimeHttpAuthConfig {
            runtime_id: "runtime-main".to_string(),
            trusted_servers: vec![TrustedServerKey {
                server_id: "server-main".to_string(),
                public_key: server.public_key.clone(),
                display_name: None,
            }],
        };

        let context = verify_capability_token(&auth, &token, Some("workers:list"), 99).unwrap();
        assert_eq!(context.workspace_id, "workspace-a");
        assert!(matches!(
            verify_capability_token(&auth, &token, Some("workers:create"), 99),
            Err(RuntimeAuthError::MissingPermission(permission)) if permission == "workers:create"
        ));
        assert!(matches!(
            verify_capability_token(&auth, &token, Some("workers:list"), 101),
            Err(RuntimeAuthError::Expired)
        ));
        let wrong_audience = RuntimeHttpAuthConfig {
            runtime_id: "other-runtime".to_string(),
            trusted_servers: auth.trusted_servers.clone(),
        };
        assert!(matches!(
            verify_capability_token(&wrong_audience, &token, Some("workers:list"), 99),
            Err(RuntimeAuthError::WrongAudience { .. })
        ));
    }
}
