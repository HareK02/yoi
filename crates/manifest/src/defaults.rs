//! Single source of truth for manifest default values.
//!
//! Every default that would otherwise be duplicated between serde
//! `#[serde(default = "...")]` attributes (on [`crate::PodManifest`])
//! and the cascade resolution in [`crate::config`] lives here as a
//! `pub const`. Both paths read from this module, so changing a
//! default requires editing exactly one line.

/// Byte-size cap applied to any tool's `content` output when no
/// per-tool override is set. See [`crate::ToolOutputLimits`].
pub const TOOL_OUTPUT_MAX_BYTES: usize = 16 * 1024;

/// Number of most-recent turns protected from pruning. See
/// [`crate::CompactionConfig::prune_protected_turns`].
pub const PRUNE_PROTECTED_TURNS: usize = 3;

/// Minimum estimated token savings required to trigger a prune. See
/// [`crate::CompactionConfig::prune_min_savings`].
pub const PRUNE_MIN_SAVINGS: u64 = 4096;

/// Number of most-recent turns retained after a compact. See
/// [`crate::CompactionConfig::compact_retained_turns`].
pub const COMPACT_RETAINED_TURNS: usize = 2;
