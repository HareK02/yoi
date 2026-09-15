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

## Metrics and comparison procedure

Compaction measurements stay out of the ordinary transcript. They are appended as
`session.metrics` extensions and are read only through the explicit
`session-metrics` reader/export path. `read_session_metrics` attaches the durable
`segment_id` and `SegmentStart.compacted_from` lineage to each record.

1. Compare runs with the same workload and model settings. Record manual versus
   automatic mode, `threshold_policy`, and `retained_token_budget`.
2. Start with the `compact.start` correlation ID. Join it to `compact.finish` and
   the metric breakdown on the result Segment, then to the next normal request's
   `compact.post_request`. Use `compacted_from`, rather than timestamps, to prove
   the source-to-result Segment relationship.
3. Compare `compact.retained_tokens`, `compact.overview_tokens`,
   `compact.summary_tokens`, `compact.auto_read_tokens`, and
   `compact.result_context_tokens` to explain the context-size change.
4. Compare the Compactor's input/output/cache-read/cache-creation tokens, request,
   turn, tool-call, and duration metrics. The current provider `UsageEvent` has no
   pricing authority, so `compact.cost_usd` is valueless with
   `status=unavailable` and `reason=provider_usage_unpriced`; do not fabricate a
   zero cost. Record a numeric value only after a price authority exists.
5. Aggregate failure and cancellation using the fixed `failure_category` values.
   Metrics must never contain provider error text, prompt/session content, host
   paths, or secrets.
6. Report at least success rate, next-request token/cache changes, Compactor token
   use, and duration per workload before changing retention or thresholds.

Export ordering is deterministic through timestamp, phase, Segment, and
`log_index`. Causality still comes from `correlation_id` and `compacted_from`, not
from timestamp order alone.
