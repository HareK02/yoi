# Compaction

Compaction exists because long-running Workers need durable continuity without sending the entire transcript forever.

## Pruning vs compaction

Pruning is request-local packing of existing history. It chooses what to include for one model request while preserving the underlying committed log.

Compaction is a durable transition. It creates a new summarized state and should be recorded so later turns can explain why older details are no longer directly present.

## Token-budget protection

Safety checks should use effective backend/context limits, including in-flight usage. Cached input tokens still occupy context even if upload displays subtract them.

When a provider returns an exact `input_total_tokens` measurement for the whole prompt shape, Yoi can treat it as authoritative and estimate only incremental growth after that measurement. It should not fake a system/history split or tune thresholds just to mask estimator bugs.

## After compaction

Compaction output is not automatically safe. The post-compact context must be revalidated before the next request.

A `just_compacted` flag must not bypass safety checks. It is easy for a compact summary, retained tail, or prompt resource change to still exceed a context limit.

## Large sessions

Large-session compaction should not send an entire prefix transcript as the summary input. Prefer bounded overview/index inputs plus exploration, then keep the retained tail small and explicit.

This keeps compaction cost predictable and avoids turning a context-recovery mechanism into the largest prompt in the session.

## History integrity

Compaction should preserve persisted reasoning history and avoid serializing unverified hidden reasoning context. Trace and metrics can count request shape and reasoning items without smuggling hidden provider state into model input.

The important property is explainability: after compaction, records should still show what summary replaced which older context and why future turns can rely on it.
