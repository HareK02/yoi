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

## Activation boundary

A compacted Segment is staged completely before it can become authoritative. The
activation boundary uses this lock order everywhere a live Worker changes Segment:

1. stop the compaction service and remove its registry record;
2. acquire the Worker's append barrier;
3. write the final pending-submission checkpoint and stage the replacement Segment;
4. acquire and preflight the machine-wide Worker allocation lock;
5. compare-and-swap `metadata.json` from the source Segment to the replacement;
6. atomically replace the allocation table while still holding its separate lock;
7. publish the append destination, session projection, sink, and in-memory history;
8. release the append barrier.

Submit and Notify acceptance take the same append barrier. Consequently an accepted
item is either durably included in the replacement checkpoint, or waits until every
live authority points at the replacement and is then appended there. Restore
admission takes the allocation lock, so it cannot observe the interval between the
metadata CAS and allocation hand-off.

The metadata CAS is the commit point. Failures before it leave the source Segment
active. A failure updating allocation after it marks the old in-memory writer
unusable and retains the machine-wide allocation lock: further acceptance fails
closed and restore admission cannot register a competing writer until process
teardown, after which restore converges from the replacement named by metadata.
The Worker must likewise remain non-idle while terminal compaction-service cleanup
is pending. Cleanup authority is stored on the Worker (not only on one compaction
future). A failed stop returns a typed cleanup-pending outcome without clearing that
authority; the next compaction boundary retries it. The authority and lifecycle
reference are cleared exactly once after the registry record is gone, before
activation or a terminal idle-capable result. While either activation repair or
cleanup attention is outstanding, the controller fences Submit/Notify dispatch and
does not start queued work. Shutdown is itself a cleanup barrier: it retries the
Worker-owned stop authority with backoff and does not emit its terminal event until
the registry record is gone.

## Metrics and comparison procedure

Compaction measurements stay out of the ordinary transcript. They are appended as
`metrics` extensions and are read only through the explicit
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
