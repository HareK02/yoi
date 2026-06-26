use crate::catalog::ProfileSelector;
use crate::error::RuntimeError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

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
    pub reference: crate::catalog::ConfigBundleRef,
    pub summary: ConfigBundleSummary,
}

pub(crate) fn validate_config_bundle(bundle: &ConfigBundle) -> Result<(), RuntimeError> {
    validate_non_empty("config bundle id", &bundle.metadata.id)?;
    validate_non_empty("config bundle digest", &bundle.metadata.digest)?;
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
        validate_non_empty("config declaration reference", &declaration.reference)?;
        validate_boundary_text("config declaration name", &declaration.name)?;
        validate_boundary_text("config declaration reference", &declaration.reference)?;
        if declaration.kind == ConfigDeclarationKind::Unsupported {
            return Err(RuntimeError::UnsupportedConfigDeclaration {
                bundle_id: bundle.metadata.id.clone(),
                declaration_kind: declaration.kind.canonical_name().to_string(),
                name: declaration.name.clone(),
            });
        }
    }
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
    if Path::new(trimmed).is_absolute()
        || trimmed.starts_with('~')
        || trimmed.contains("/.cache")
        || trimmed.contains("\\.cache")
        || trimmed.contains("/run/")
        || trimmed.contains("\\run\\")
        || trimmed.contains(".sock")
        || trimmed.contains("socket=")
        || trimmed.contains("session_path")
        || trimmed.contains("cache_path")
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
