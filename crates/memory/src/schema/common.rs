//! Common frontmatter helpers and shared types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::LintError;

pub use lint_common::Frontmatter;

/// Reference to a session-store entry range. Stored in `sources` /
/// `last_sources` arrays for traceability back to raw session logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceRef {
    pub segment_id: String,
    /// `[start_entry, end_entry]` inclusive range of session-store entry indices.
    pub range: [u64; 2],
}

impl<'de> Deserialize<'de> for SourceRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawSourceRef {
            #[serde(default)]
            segment_id: Option<String>,
            #[serde(default)]
            session_id: Option<String>,
            range: [u64; 2],
        }

        let raw = RawSourceRef::deserialize(deserializer)?;
        let segment_id = raw
            .segment_id
            .or(raw.session_id)
            .ok_or_else(|| serde::de::Error::missing_field("segment_id"))?;
        Ok(SourceRef {
            segment_id,
            range: raw.range,
        })
    }
}

/// Extensible evidence kind tag used by staging source anchors.
///
/// Known values include `message`, `tool_call`, `tool_result`, `file_ref`,
/// `ticket_ref`, and `objective_ref`, but callers may use newer bounded tags
/// without changing the schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceKind(pub String);

impl EvidenceKind {
    pub const MESSAGE: &'static str = "message";
    pub const TOOL_CALL: &'static str = "tool_call";
    pub const TOOL_RESULT: &'static str = "tool_result";
    pub const FILE_REF: &'static str = "file_ref";
    pub const TICKET_REF: &'static str = "ticket_ref";
    pub const OBJECTIVE_REF: &'static str = "objective_ref";

    pub fn new(kind: impl Into<String>) -> Self {
        Self(kind.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceOriginKind {
    HumanInput,
    WorkerInput,
    FlowInstruction,
    BackendInstruction,
    ModelOutput,
    ToolOutput,
    DerivedSummary,
    LegacyUnknown,
}

/// Bounded origin snapshot attached to extraction evidence. This is audit
/// metadata only and cannot authorize Workspace operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceOrigin {
    pub kind: EvidenceOriginKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_definition_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_definition_revision: Option<u64>,
}

/// Host-resolved source/evidence metadata for an individual staging claim.
///
/// This deliberately stores only bounded anchor metadata: stable ids, entry
/// ranges, and short labels/summaries. It must not carry raw message bodies or
/// full tool result content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceEvidenceRef {
    /// Stable session id when the anchor crosses or disambiguates segments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Session segment id containing the anchored log entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<String>,
    /// `[start_entry, end_entry]` inclusive range of session-store entry indices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_range: Option<[u64; 2]>,
    /// Host-assigned evidence id within the referenced evidence set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_id: Option<String>,
    /// Trusted typed origin snapshot for this logical evidence entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<EvidenceOrigin>,
    /// Extensible evidence kind tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_kind: Option<EvidenceKind>,
    /// Short host-provided display label, not raw evidence content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Short host-provided summary, not raw evidence content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Split a markdown document into `(yaml_frontmatter, body)`.
pub fn split_frontmatter(content: &str) -> Result<(&str, &str), LintError> {
    lint_common::split_frontmatter(content).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lint_common::RecordLintError;

    #[test]
    fn splits_simple() {
        let doc = "---\nfoo: 1\n---\nbody here\n";
        let (y, b) = split_frontmatter(doc).unwrap();
        assert_eq!(y, "foo: 1\n");
        assert_eq!(b, "body here\n");
    }

    #[test]
    fn no_leading_delim_errors() {
        let err = split_frontmatter("hello").unwrap_err();
        assert!(matches!(
            err,
            LintError::Record(RecordLintError::MissingFrontmatter)
        ));
    }

    #[test]
    fn no_closing_delim_errors() {
        let err = split_frontmatter("---\nfoo: 1\nno close\n").unwrap_err();
        assert!(matches!(
            err,
            LintError::Record(RecordLintError::MalformedFrontmatter(_))
        ));
    }

    #[test]
    fn handles_empty_body() {
        let doc = "---\nfoo: 1\n---\n";
        let (_, b) = split_frontmatter(doc).unwrap();
        assert_eq!(b, "");
    }
}
