use crate::catalog::{ConfigBundleRef, ProfileSelector};
use crate::error::RuntimeError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CONFIG_BUNDLE_DIGEST_ALGORITHM: &str = "sha256";

/// Backend-synced Profile/config bundle stored by a Runtime.
///
/// The bundle is intentionally an intent/declaration boundary: it contains
/// profile selectors plus refs/grants/policies, never secret values, direct
/// Runtime endpoints, raw socket/session paths, runtime-local mount actual
/// paths, host-local cache paths, or fully resolved WorkerSpec content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigBundle {
    pub metadata: ConfigBundleMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<ConfigProfileDescriptor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declarations: Vec<ConfigDeclaration>,
}

impl ConfigBundle {
    pub fn computed_digest(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("id\0{}", self.metadata.id));
        lines.push(format!("revision\0{}", self.metadata.revision));
        lines.push(format!("workspace_id\0{}", self.metadata.workspace_id));
        lines.push(format!("created_at\0{}", self.metadata.created_at));
        lines.push(format!(
            "provenance.source\0{}",
            self.metadata.provenance.source
        ));
        lines.push(format!(
            "provenance.detail\0{}",
            self.metadata.provenance.detail.as_deref().unwrap_or("")
        ));

        let mut profiles = self.profiles.clone();
        profiles.sort_by(|left, right| {
            profile_sort_key(&left.selector).cmp(&profile_sort_key(&right.selector))
        });
        for profile in profiles {
            lines.push(format!(
                "profile\0{}\0{}",
                profile_sort_key(&profile.selector),
                profile.label.unwrap_or_default()
            ));
        }

        let mut declarations = self.declarations.clone();
        declarations
            .sort_by(|left, right| declaration_sort_key(left).cmp(&declaration_sort_key(right)));
        for declaration in declarations {
            lines.push(format!(
                "declaration\0{}\0{}\0{}",
                declaration.kind.canonical_name(),
                declaration.name,
                declaration.reference
            ));
        }

        lines.sort();
        let mut hasher = Sha256::new();
        for line in lines {
            hasher.update(line.as_bytes());
            hasher.update(b"\n");
        }
        let digest = hasher.finalize();
        hex_digest(&digest)
    }

    pub fn with_computed_digest(mut self) -> Self {
        self.metadata.digest = self.computed_digest();
        self
    }

    pub fn summary(&self) -> ConfigBundleSummary {
        ConfigBundleSummary {
            id: self.metadata.id.clone(),
            digest: self.metadata.digest.clone(),
            digest_algorithm: CONFIG_BUNDLE_DIGEST_ALGORITHM.to_string(),
            revision: self.metadata.revision.clone(),
            workspace_id: self.metadata.workspace_id.clone(),
            created_at: self.metadata.created_at.clone(),
            provenance: self.metadata.provenance.clone(),
            profile_count: self.profiles.len(),
            declaration_count: self.declarations.len(),
        }
    }

    pub fn contains_profile(&self, selector: &ProfileSelector) -> bool {
        self.profiles
            .iter()
            .any(|profile| profile.selector == *selector)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigBundleMetadata {
    pub id: String,
    pub digest: String,
    pub revision: String,
    pub workspace_id: String,
    pub created_at: String,
    pub provenance: ConfigBundleProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigBundleProvenance {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigProfileDescriptor {
    pub selector: ProfileSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigDeclaration {
    pub kind: ConfigDeclarationKind,
    pub name: String,
    pub reference: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigDeclarationKind {
    SecretRef,
    MountGrant,
    NetworkPolicy,
    ShellPolicy,
    GitPolicy,
    CapabilityGrant,
    Unsupported,
}

impl ConfigDeclarationKind {
    pub fn canonical_name(&self) -> &'static str {
        match self {
            Self::SecretRef => "secret_ref",
            Self::MountGrant => "mount_grant",
            Self::NetworkPolicy => "network_policy",
            Self::ShellPolicy => "shell_policy",
            Self::GitPolicy => "git_policy",
            Self::CapabilityGrant => "capability_grant",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigBundleSummary {
    pub id: String,
    pub digest: String,
    pub digest_algorithm: String,
    pub revision: String,
    pub workspace_id: String,
    pub created_at: String,
    pub provenance: ConfigBundleProvenance,
    pub profile_count: usize,
    pub declaration_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigBundleAvailability {
    pub reference: ConfigBundleRef,
    pub summary: ConfigBundleSummary,
}

pub(crate) fn validate_config_bundle(bundle: &ConfigBundle) -> Result<(), RuntimeError> {
    validate_config_bundle_id(&bundle.metadata.id)?;
    validate_non_empty("config bundle digest", &bundle.metadata.digest)?;
    validate_digest("config bundle digest", &bundle.metadata.digest)?;
    validate_non_empty("config bundle revision", &bundle.metadata.revision)?;
    validate_non_empty("config bundle workspace id", &bundle.metadata.workspace_id)?;
    validate_non_empty("config bundle created_at", &bundle.metadata.created_at)?;
    validate_non_empty(
        "config bundle provenance source",
        &bundle.metadata.provenance.source,
    )?;
    validate_boundary_text("config bundle id", &bundle.metadata.id)?;
    validate_boundary_text("config bundle revision", &bundle.metadata.revision)?;
    validate_boundary_text("config bundle workspace id", &bundle.metadata.workspace_id)?;
    validate_boundary_text(
        "config bundle provenance source",
        &bundle.metadata.provenance.source,
    )?;
    if let Some(detail) = &bundle.metadata.provenance.detail {
        validate_boundary_text("config bundle provenance detail", detail)?;
    }

    let computed = bundle.computed_digest();
    if computed != bundle.metadata.digest {
        return Err(RuntimeError::ConfigBundleDigestMismatch {
            bundle_id: bundle.metadata.id.clone(),
            expected_digest: bundle.metadata.digest.clone(),
            actual_digest: computed,
        });
    }

    for profile in &bundle.profiles {
        validate_profile_selector(profile.selector.clone(), Some(&bundle.metadata.id))?;
        if let Some(label) = &profile.label {
            validate_boundary_text("profile label", label)?;
        }
    }

    for declaration in &bundle.declarations {
        validate_non_empty("config declaration name", &declaration.name)?;
        validate_boundary_text("config declaration name", &declaration.name)?;
        if declaration.kind == ConfigDeclarationKind::Unsupported {
            return Err(RuntimeError::UnsupportedConfigDeclaration {
                bundle_id: bundle.metadata.id.clone(),
                declaration_kind: declaration.kind.canonical_name().to_string(),
                name: declaration.name.clone(),
            });
        }
        validate_declaration_reference(&bundle.metadata.id, declaration)?;
    }
    Ok(())
}

pub(crate) fn validate_config_bundle_ref(reference: &ConfigBundleRef) -> Result<(), RuntimeError> {
    validate_config_bundle_id(&reference.id)?;
    validate_non_empty("config bundle reference digest", &reference.digest)?;
    validate_digest("config bundle reference digest", &reference.digest)?;
    Ok(())
}

pub(crate) fn validate_profile_selector(
    selector: ProfileSelector,
    bundle_id: Option<&str>,
) -> Result<(), RuntimeError> {
    match selector {
        ProfileSelector::RuntimeDefault => Ok(()),
        ProfileSelector::Builtin(value) | ProfileSelector::Named(value) => {
            if value.trim().is_empty() {
                Err(RuntimeError::InvalidProfileSelector {
                    profile: value,
                    bundle_id: bundle_id.map(ToOwned::to_owned),
                    message: "profile selector must not be empty".to_string(),
                })
            } else {
                validate_boundary_text("profile selector", &value).map_err(|err| match err {
                    RuntimeError::InvalidRequest(message) => RuntimeError::InvalidProfileSelector {
                        profile: value,
                        bundle_id: bundle_id.map(ToOwned::to_owned),
                        message,
                    },
                    other => other,
                })
            }
        }
    }
}

fn validate_non_empty(label: &'static str, value: &str) -> Result<(), RuntimeError> {
    if value.trim().is_empty() {
        Err(RuntimeError::InvalidRequest(format!(
            "{label} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn validate_config_bundle_id(value: &str) -> Result<(), RuntimeError> {
    validate_non_empty("config bundle id", value)?;
    let trimmed = value.trim();
    if trimmed.len() > 128 {
        return Err(RuntimeError::InvalidRequest(
            "config bundle id is too large".to_string(),
        ));
    }
    if trimmed != value {
        return Err(RuntimeError::InvalidRequest(
            "config bundle id must not contain surrounding whitespace".to_string(),
        ));
    }
    if !trimmed
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(RuntimeError::InvalidRequest(
            "config bundle id must be a path-safe stable identifier".to_string(),
        ));
    }
    Ok(())
}

fn validate_digest(label: &'static str, value: &str) -> Result<(), RuntimeError> {
    let trimmed = value.trim();
    if trimmed != value
        || trimmed.len() != 64
        || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RuntimeError::InvalidRequest(format!(
            "{label} must be a 64-character lowercase sha256 hex digest"
        )));
    }
    if !trimmed
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(RuntimeError::InvalidRequest(format!(
            "{label} must be a 64-character lowercase sha256 hex digest"
        )));
    }
    Ok(())
}

fn validate_declaration_reference(
    bundle_id: &str,
    declaration: &ConfigDeclaration,
) -> Result<(), RuntimeError> {
    validate_non_empty("config declaration reference", &declaration.reference)?;
    validate_ref_boundary_text("config declaration reference", &declaration.reference)?;
    let allowed_prefixes: &[&str] = match declaration.kind {
        ConfigDeclarationKind::SecretRef => &["secret:", "secret-ref:", "vault:", "keyring:"],
        ConfigDeclarationKind::MountGrant => &["mount:", "mount-grant:"],
        ConfigDeclarationKind::NetworkPolicy => &["network:", "network-policy:"],
        ConfigDeclarationKind::ShellPolicy => &["shell:", "shell-policy:"],
        ConfigDeclarationKind::GitPolicy => &["git:", "git-policy:"],
        ConfigDeclarationKind::CapabilityGrant => &["capability:", "capability-grant:"],
        ConfigDeclarationKind::Unsupported => &[],
    };
    if !allowed_prefixes.iter().any(|prefix| {
        declaration.reference.starts_with(prefix) && declaration.reference.len() > prefix.len()
    }) {
        return Err(RuntimeError::UnsupportedConfigDeclaration {
            bundle_id: bundle_id.to_string(),
            declaration_kind: declaration.kind.canonical_name().to_string(),
            name: declaration.name.clone(),
        });
    }
    Ok(())
}

fn validate_ref_boundary_text(label: &'static str, value: &str) -> Result<(), RuntimeError> {
    let trimmed = value.trim();
    validate_boundary_text(label, trimmed)?;
    if trimmed != value
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('?')
        || trimmed.contains('&')
        || trimmed.contains('#')
        || trimmed.contains('%')
        || trimmed.contains('=')
        || trimmed.chars().any(char::is_whitespace)
        || !trimmed.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.' | b'@' | b'+')
        })
    {
        return Err(RuntimeError::InvalidRequest(format!(
            "{label} must be a typed ref/grant/policy token, not a secret value or path"
        )));
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains(".cache")
        || lower.contains(".yoi")
        || lower.contains(".sock")
        || lower.contains("socket=")
        || lower.contains("session_path")
        || lower.contains("cache_path")
    {
        return Err(RuntimeError::InvalidRequest(format!(
            "{label} must not contain host-local cache/session/socket material"
        )));
    }
    Ok(())
}

fn validate_boundary_text(label: &'static str, value: &str) -> Result<(), RuntimeError> {
    let trimmed = value.trim();
    if trimmed.len() > 2048 {
        return Err(RuntimeError::InvalidRequest(format!(
            "{label} is too large"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(RuntimeError::InvalidRequest(format!(
            "{label} must not contain control characters"
        )));
    }
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.starts_with('/')
        || trimmed.starts_with('~')
        || trimmed.contains(":\\")
        || lower.contains(".cache")
        || lower.contains(".yoi/sessions")
        || lower.contains(".yoi\\sessions")
        || lower.contains("/sessions/")
        || lower.contains("\\sessions\\")
        || lower.contains("/run/")
        || lower.contains("\\run\\")
        || lower.contains(".sock")
        || lower.contains("/sock")
        || lower.contains("\\sock")
        || lower.contains("socket=")
        || lower.contains("session_path")
        || lower.contains("cache_path")
    {
        return Err(RuntimeError::InvalidRequest(format!(
            "{label} must be a stable ref/grant/policy declaration, not a host-local path"
        )));
    }
    Ok(())
}

fn declaration_sort_key(declaration: &ConfigDeclaration) -> String {
    format!(
        "{}\0{}\0{}",
        declaration.kind.canonical_name(),
        declaration.name,
        declaration.reference
    )
}

fn profile_sort_key(selector: &ProfileSelector) -> String {
    match selector {
        ProfileSelector::RuntimeDefault => "runtime_default".to_string(),
        ProfileSelector::Builtin(value) => format!("builtin\0{value}"),
        ProfileSelector::Named(value) => format!("named\0{value}"),
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle_with_declaration(reference: &str) -> ConfigBundle {
        ConfigBundle {
            metadata: ConfigBundleMetadata {
                id: "bundle-1".to_string(),
                digest: String::new(),
                revision: "rev-1".to_string(),
                workspace_id: "workspace-1".to_string(),
                created_at: "2026-06-26T00:00:00Z".to_string(),
                provenance: ConfigBundleProvenance {
                    source: "test".to_string(),
                    detail: None,
                },
            },
            profiles: vec![ConfigProfileDescriptor {
                selector: ProfileSelector::Builtin("builtin:coder".to_string()),
                label: None,
            }],
            declarations: vec![ConfigDeclaration {
                kind: ConfigDeclarationKind::SecretRef,
                name: "credential".to_string(),
                reference: reference.to_string(),
            }],
        }
        .with_computed_digest()
    }

    #[test]
    fn rejects_host_local_cache_session_socket_and_plaintext_secret_refs() {
        for reference in [
            ".cache/yoi",
            ".yoi/sessions/foo.jsonl",
            "pods/foo/sock",
            "password=hunter2",
            "hunter2-secret-value",
        ] {
            let error = validate_config_bundle(&bundle_with_declaration(reference)).unwrap_err();
            assert!(
                matches!(
                    error,
                    RuntimeError::InvalidRequest(_)
                        | RuntimeError::UnsupportedConfigDeclaration { .. }
                ),
                "unexpected error for {reference}: {error:?}"
            );
        }
    }

    #[test]
    fn accepts_typed_secret_refs() {
        validate_config_bundle(&bundle_with_declaration("secret:github-token")).unwrap();
        validate_config_bundle(&bundle_with_declaration("vault:team.api-key")).unwrap();
    }

    #[test]
    fn rejects_unsafe_bundle_ids_and_refs() {
        for id in ["bundle/1", "bundle?x", "bundle&x", "bundle#x", " bundle"] {
            let mut bundle = bundle_with_declaration("secret:github-token");
            bundle.metadata.id = id.to_string();
            bundle = bundle.with_computed_digest();
            assert!(validate_config_bundle(&bundle).is_err(), "accepted id {id}");
        }

        assert!(
            validate_config_bundle_ref(&ConfigBundleRef {
                id: "bundle/1".to_string(),
                digest: "0".repeat(64),
            })
            .is_err()
        );
    }
}
