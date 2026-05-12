//! Memory-scoped tools: Read / Write / Edit / Search.
//!
//! All four take `kind` + `slug` (Summary takes only `kind`) and
//! resolve the path through [`WorkspaceLayout`]. The agent never has
//! to know the on-disk layout — Search returns `{slug, kind, ...}` and
//! that pair feeds straight into Read / Edit.

mod edit;
mod query;
mod read;
mod write;

use std::path::PathBuf;

use llm_worker::tool::ToolError;
use serde::Deserialize;

use crate::Slug;
use crate::workspace::{RecordKind, WorkspaceLayout};

pub use edit::edit_tool;
pub use query::{QueryConfig, knowledge_query_tool, memory_query_tool};
pub use read::{read_tool, read_tool_with_usage};
pub use write::write_tool;

/// Kinds the memory tools accept as input. `Workflow` is intentionally
/// excluded — workflows are sub-Worker context, not agent-editable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemoryToolKind {
    Summary,
    Decision,
    Request,
    Knowledge,
}

impl MemoryToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Decision => "decision",
            Self::Request => "request",
            Self::Knowledge => "knowledge",
        }
    }

    pub fn record_kind(self) -> RecordKind {
        match self {
            Self::Summary => RecordKind::Summary,
            Self::Decision => RecordKind::Decision,
            Self::Request => RecordKind::Request,
            Self::Knowledge => RecordKind::Knowledge,
        }
    }

    /// Resolve `(kind, slug)` to an absolute path under the workspace.
    /// Summary forbids a slug; the per-record kinds require one.
    pub fn resolve_path(
        self,
        layout: &WorkspaceLayout,
        slug: Option<&str>,
    ) -> Result<PathBuf, ToolError> {
        match self {
            Self::Summary => {
                if slug.is_some() {
                    return Err(ToolError::InvalidArgument(
                        "kind=summary does not accept a slug".to_string(),
                    ));
                }
                Ok(layout.summary_path())
            }
            other => {
                let raw = slug.ok_or_else(|| {
                    ToolError::InvalidArgument(format!("kind={} requires `slug`", other.as_str()))
                })?;
                let parsed =
                    Slug::parse(raw).map_err(|e| ToolError::InvalidArgument(e.to_string()))?;
                Ok(match other {
                    Self::Decision => layout.decision_path(&parsed),
                    Self::Request => layout.request_path(&parsed),
                    Self::Knowledge => layout.knowledge_path(&parsed),
                    Self::Summary => unreachable!(),
                })
            }
        }
    }
}
