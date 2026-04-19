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

/// Token budget retained (unchanged) at the tail of the history across
/// a compact. Items whose cumulative token count fits within this budget
/// starting from the end are kept verbatim; the rest are summarised.
/// See [`crate::CompactionConfig::compact_retained_tokens`].
pub const COMPACT_RETAINED_TOKENS: u64 = 8000;

/// Default instruction asset reference used when `worker.instruction`
/// is omitted. See the `PromptLoader` prefix addressing scheme for the
/// `$insomnia/` / `$user/` / `$workspace/` namespaces.
pub const DEFAULT_INSTRUCTION: &str = "$insomnia/default";

/// Token budget for auto-read file contents injected into the new
/// session after compaction. Limits how much raw file text the
/// compact worker can pull into the compacted context via
/// `mark_read_required`. See
/// [`crate::CompactionConfig::compact_auto_read_budget`].
pub const COMPACT_AUTO_READ_BUDGET: u64 = 8000;

/// Cumulative input-token cap for the compact worker's own LLM
/// calls. Exceeding this aborts the compact run (circuit-breaker
/// path). See
/// [`crate::CompactionConfig::compact_worker_max_input_tokens`].
pub const COMPACT_WORKER_MAX_INPUT_TOKENS: u64 = 50_000;

/// Number of recently-touched files fed to the compact worker as
/// default references.
pub const COMPACT_DEFAULT_REFERENCE_COUNT: usize = 5;
