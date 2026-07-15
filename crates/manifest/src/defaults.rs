//! Single source of truth for manifest default values.
//!
//! Every default that would otherwise be duplicated between serde
//! `#[serde(default = "...")]` attributes (on [`crate::WorkerManifest`])
//! and the cascade resolution in [`crate::config`] lives here as a
//! `pub const`. Both paths read from this module, so changing a
//! default requires editing exactly one line.

/// Byte-size cap applied to any tool's `content` output when no
/// per-tool override is set. See [`crate::ToolOutputLimits`].
pub const TOOL_OUTPUT_MAX_BYTES: usize = 64 * 1024;

/// Byte-size cap applied to each submit-time FileRef upload / attachment.
/// See [`crate::FileUploadLimits`].
pub const FILE_UPLOAD_MAX_BYTES: usize = 256 * 1024;

/// Token budget at the history tail protected from pruning. See
/// [`crate::CompactionConfig::prune_protected_tokens`].
pub const PRUNE_PROTECTED_TOKENS: u64 = 8000;

/// Minimum estimated token savings required to trigger a prune. See
/// [`crate::CompactionConfig::prune_min_savings`].
pub const PRUNE_MIN_SAVINGS: u64 = 4096;

/// Token budget retained (unchanged) at the tail of the history across
/// a compact. Items whose cumulative token count fits within this budget
/// starting from the end are kept verbatim; the rest are summarised.
/// See [`crate::CompactionConfig::retained_tokens`].
pub const COMPACT_RETAINED_TOKENS: u64 = 8000;

/// Target size for the deterministic compact overview/index fed to the
/// compact worker. Exceeding this target is tolerated.
/// See [`crate::CompactionConfig::overview_target_tokens`].
pub const COMPACT_OVERVIEW_TARGET_TOKENS: u64 = 8_000;

/// Warning threshold for compact overview/index size. Compaction continues.
/// See [`crate::CompactionConfig::overview_warning_tokens`].
pub const COMPACT_OVERVIEW_WARNING_TOKENS: u64 = 16_000;

/// Hard deterministic-overview deadline. When exceeded, overview generation
/// falls back to a coarser index before the compact worker is started.
/// See [`crate::CompactionConfig::overview_deadline_tokens`].
pub const COMPACT_OVERVIEW_DEADLINE_TOKENS: u64 = 40_000;

/// Default instruction asset reference used when `worker.instruction`
/// is omitted. See the `PromptLoader` prefix addressing scheme for the
/// `$yoi/` / `$user/` / `$workspace/` namespaces.
pub const DEFAULT_INSTRUCTION: &str = "$yoi/default";

/// Default language policy used by the main worker for normal prose
/// responses. See [`crate::EngineManifest::language`].
pub const WORKER_LANGUAGE: &str =
    "match the user's language unless they explicitly request another language";

/// Token budget for auto-read file contents injected into the new
/// session after compaction. Limits how much raw file text the
/// compact worker can pull into the compacted context via
/// `mark_read_required`. See
/// [`crate::CompactionConfig::auto_read_budget_tokens`].
pub const COMPACT_AUTO_READ_BUDGET: u64 = 8000;

/// Current prompt-occupancy cap for the compact worker's own LLM
/// calls. Exceeding this aborts the compact run (circuit-breaker
/// path). See [`crate::CompactionConfig::worker_context_max_tokens`].
pub const COMPACT_WORKER_MAX_INPUT_TOKENS: u64 = 50_000;

/// Remaining compact-worker context threshold that triggers an instruction
/// to stop exploring and call `write_summary`.
/// See [`crate::CompactionConfig::finish_warning_remaining_tokens`].
pub const COMPACT_FINISH_WARNING_REMAINING_TOKENS: u64 = 8_000;

/// Context reserve preserved for final summary/tool closing turns.
/// See [`crate::CompactionConfig::final_reserve_tokens`].
pub const COMPACT_FINAL_RESERVE_TOKENS: u64 = 4_000;

/// Optional maximum compact-worker tool-loop depth. `None` means unlimited.
/// See [`crate::CompactionConfig::worker_max_turns`].
pub const COMPACT_WORKER_MAX_TURNS: Option<u32> = Some(20);

/// Target size for the `write_summary` text. Used in prompt/nudge text.
/// See [`crate::CompactionConfig::summary_target_tokens`].
pub const COMPACT_SUMMARY_TARGET_TOKENS: u64 = 2_000;

/// Hard validation cap for the final `write_summary` text.
/// See [`crate::CompactionConfig::summary_max_tokens`].
pub const COMPACT_SUMMARY_MAX_TOKENS: u64 = 4_000;

/// Dry-run cap for the compacted session's initial request context.
/// See [`crate::CompactionConfig::result_context_max_tokens`].
pub const COMPACT_RESULT_CONTEXT_MAX_TOKENS: u64 = 60_000;

/// Number of recently-touched files fed to the compact worker as
/// default references.
pub const COMPACT_DEFAULT_REFERENCE_COUNT: usize = 5;

/// Optional maximum extract-worker tool-loop depth. `None` means unlimited.
/// See [`crate::MemoryConfig::extract_worker_max_turns`].
pub const MEMORY_EXTRACT_WORKER_MAX_TURNS: Option<u32> = Some(8);

/// Default language used by memory extraction / consolidation workers for
/// durable memory text. See [`crate::MemoryConfig::language`].
pub const MEMORY_LANGUAGE: &str = "English";
