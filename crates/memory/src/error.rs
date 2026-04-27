//! Errors raised by the memory subsystem.

use std::path::PathBuf;

use thiserror::Error;

/// Top-level error for memory operations that don't fit the lint flow.
#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("path is not under the memory or knowledge tree: {}", .0.display())]
    OutsideMemoryTree(PathBuf),
    #[error("path is not absolute: {}", .0.display())]
    RelativePath(PathBuf),
    #[error("io error at {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl MemoryError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// A single Linter violation. Multiple are aggregated in a [`LintReport`].
///
/// `Display` produces a one-line message used directly in the `ToolError`
/// payload returned to the LLM.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum LintError {
    #[error("path is not a valid memory record location: {}", .0.display())]
    InvalidPath(PathBuf),

    #[error("path is for a different record kind than expected at this location: {}", .0.display())]
    WrongRecordKind(PathBuf),

    #[error("invalid slug `{0}`: must match ^[a-z0-9](?:[a-z0-9-]{{0,62}}[a-z0-9])?$")]
    InvalidSlug(String),

    #[error("malformed frontmatter: {0}")]
    MalformedFrontmatter(String),

    #[error("frontmatter is missing or document is empty")]
    MissingFrontmatter,

    #[error("missing required frontmatter field: `{0}`")]
    MissingField(&'static str),

    #[error("invalid value for `{field}`: {message}")]
    InvalidField {
        field: &'static str,
        message: String,
    },

    #[error("Decisions `status` must be one of open|resolved|replaced (got `{0}`)")]
    InvalidStatus(String),

    #[error(
        "Knowledge with model_invokation: true cannot have description longer than {limit} chars (got {actual})"
    )]
    DescriptionTooLong { actual: usize, limit: usize },

    #[error("body exceeds the size limit for this record kind: {actual} chars > {limit}")]
    BodyTooLong { actual: usize, limit: usize },

    #[error(
        "write to `memory/workflow/` is forbidden via the memory tool — Workflows are human-edited"
    )]
    WorkflowWriteForbidden,

    #[error("slug `{0}` already exists; use the edit tool instead of creating a new record")]
    SlugAlreadyExists(String),

    #[error("`{field}` references unknown {kind} slug `{slug}`")]
    UnknownReference {
        field: &'static str,
        kind: &'static str,
        slug: String,
    },

    #[error("`replaced_by` chain forms a cycle: {chain}")]
    ReplacedByCycle { chain: String },

    #[error("`replaced_by` must point to a different slug than the record itself")]
    ReplacedBySelf,
}

/// A single Linter warning (non-blocking).
///
/// Warnings ride along in the `ToolOutput.summary` so the agent can act
/// on them when convenient; they never abort the write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LintWarning {
    /// Single-source record exceeds the importance/size threshold.
    LowImportanceLargeRecord { chars: usize },
    /// `sources` array has grown past the soft cap.
    SourcesOverflow { count: usize },
    /// Multiple slugs in the same kind are within Levenshtein distance 2.
    SimilarSlugs(Vec<String>),
}

impl std::fmt::Display for LintWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LowImportanceLargeRecord { chars } => write!(
                f,
                "record is large ({chars} chars) but only has 1 source — consider splitting or trimming"
            ),
            Self::SourcesOverflow { count } => write!(
                f,
                "`sources` has {count} entries — consider keeping only the most recent and relying on git log for the rest"
            ),
            Self::SimilarSlugs(slugs) => {
                write!(f, "similar slugs detected (consider merging): ")?;
                for (i, s) in slugs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{s}")?;
                }
                Ok(())
            }
        }
    }
}
