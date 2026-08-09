use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FlowSelector {
    Builtin { slug: String },
    Workspace { slug: String },
}

impl FlowSelector {
    pub fn builtin(slug: impl Into<String>) -> Result<Self, FlowSelectorError> {
        let slug = slug.into();
        validate_slug(&slug)?;
        Ok(Self::Builtin { slug })
    }

    pub fn workspace(slug: impl Into<String>) -> Result<Self, FlowSelectorError> {
        let slug = slug.into();
        validate_slug(&slug)?;
        Ok(Self::Workspace { slug })
    }

    pub fn slug(&self) -> &str {
        match self {
            Self::Builtin { slug } | Self::Workspace { slug } => slug,
        }
    }

    pub fn source_kind(&self) -> FlowSourceKind {
        match self {
            Self::Builtin { .. } => FlowSourceKind::Builtin,
            Self::Workspace { .. } => FlowSourceKind::Workspace,
        }
    }
}

impl fmt::Display for FlowSelector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin { slug } => write!(formatter, "builtin:{slug}"),
            Self::Workspace { slug } => write!(formatter, "workspace:{slug}"),
        }
    }
}

impl FromStr for FlowSelector {
    type Err = FlowSelectorError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (source, slug) = value.split_once(':').ok_or_else(|| {
            FlowSelectorError::InvalidFormat(
                "Flow selector must be source-qualified as builtin:<slug> or workspace:<slug>"
                    .to_string(),
            )
        })?;
        if slug.contains(':') {
            return Err(FlowSelectorError::InvalidFormat(
                "Flow selector must contain exactly one ':' separator".to_string(),
            ));
        }
        match source {
            "builtin" => Self::builtin(slug),
            "workspace" => Self::workspace(slug),
            other => Err(FlowSelectorError::UnknownSource(other.to_string())),
        }
    }
}

impl Serialize for FlowSelector {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for FlowSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowSourceKind {
    Builtin,
    Workspace,
}

impl FlowSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Workspace => "workspace",
        }
    }
}

impl fmt::Display for FlowSourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowSourceResolveRequest {
    pub selector: FlowSelector,
}

/// Immutable source snapshot resolved by Workspace authority for one Runtime.
///
/// This is read-only source authority. Starting or mutating a Flow instance is
/// deliberately not part of this response; Runtime persists the snapshot in
/// the target Worker's durable state before execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedFlowSource {
    pub selector: FlowSelector,
    pub workspace_id: String,
    pub flow_id: String,
    pub revision: u64,
    pub content_digest: String,
    pub definition: crate::CompiledFlowDefinition,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum FlowSelectorError {
    #[error("{0}")]
    InvalidFormat(String),
    #[error("unknown Flow selector source {0:?}")]
    UnknownSource(String),
    #[error("invalid Flow selector slug: {0}")]
    InvalidSlug(String),
}

fn validate_slug(slug: &str) -> Result<(), FlowSelectorError> {
    if slug.is_empty() {
        return Err(FlowSelectorError::InvalidSlug(
            "slug must not be empty".to_string(),
        ));
    }
    if slug.len() > 128 {
        return Err(FlowSelectorError::InvalidSlug(
            "slug exceeds 128 bytes".to_string(),
        ));
    }
    if !slug
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(FlowSelectorError::InvalidSlug(
            "slug must contain only ASCII letters, digits, '-' or '_'".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_requires_explicit_known_source() {
        assert_eq!(
            "builtin:coder-review".parse::<FlowSelector>().unwrap(),
            FlowSelector::Builtin {
                slug: "coder-review".to_string()
            }
        );
        assert_eq!(
            "workspace:coder-review"
                .parse::<FlowSelector>()
                .unwrap()
                .to_string(),
            "workspace:coder-review"
        );
        assert!("coder-review".parse::<FlowSelector>().is_err());
        assert!("project:coder-review".parse::<FlowSelector>().is_err());
        assert!("builtin:bad/path".parse::<FlowSelector>().is_err());
        assert!("builtin:a:b".parse::<FlowSelector>().is_err());
    }

    #[test]
    fn selector_serde_is_one_canonical_string() {
        let selector = FlowSelector::builtin("coder-review").unwrap();
        let json = serde_json::to_string(&selector).unwrap();
        assert_eq!(json, r#""builtin:coder-review""#);
        assert_eq!(
            serde_json::from_str::<FlowSelector>(&json).unwrap(),
            selector
        );
    }
}
