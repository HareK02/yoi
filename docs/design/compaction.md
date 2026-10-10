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

`result_context_max_tokens` is an advisory warning threshold for the assembled replacement
history, not a hard activation cap or a provider context limit. Its estimate includes the summary,
auto-read files, references, retained tail, and TaskStore context. Exceeding it emits a Compactor
warning and continues without discarding the generated summary or stripping auto-read/retained
content. `0` disables this warning. Normal request-time safety checks still apply after activation.

## Compactor inputs and continuity policy

The compactor receives the previous Host-produced summary in full as mandatory source
material, separately from the bounded overview of this Segment's ordinary prefix. It must
preserve ongoing user goals, directives, completion conditions, and unresolved work from
that summary and from the current Segment's user messages. Recent progress and the
TaskStore snapshot are supporting evidence, not replacements for the original request.
Important wording or ambiguous completion state should be checked through exploration
rather than reconstructed from recent activity.

The first model input includes:

- `## Initial compaction summary (full text)`, when a prior summary exists, containing its
  complete body without overview truncation;
- recently touched file references, when available;
- the full current Session TaskStore snapshot;
- `## Conversation overview/index`, a bounded index of the ordinary prefix selected for
  summarization, with exploration tools for omitted details.

The inherited summary is selected by `LoggedSessionHistoryOrigin::CompactionSummary`,
not by message role or pasted text. Only the Host-produced opening compaction summary
receives that origin. Auto-read contents, related-file context, and TaskStore snapshots
remain `DerivedSummary` and follow ordinary prefix/retention policy. A user or system
message containing `[Compacted context summary]` is not sufficient to qualify.

Restore recognizes one explicitly adopted legacy construction: the first history item of
an `AnnotatedSegmentStart` with `compacted_from: Some(...)`, whose origin is
`DerivedSummary`, whose role is System, and whose text starts with the exact Host summary
marker. That item is interpreted as `CompactionSummary` in restored history. Restore does
not promote later derived items, user messages, unknown origins, or marker-bearing items
without compacted lineage, and does not rewrite the original durable record.

## Retained tail

The retained-token budget selects an ordinary history tail, at item granularity. Retained
entries remain in the replacement Segment with their item and metadata preserved, except
for documented binary-attachment projection. They are outside the prefix overview.

The previous `CompactionSummary` is an explicit exception: its full body is supplied to the
compactor regardless of whether the cut lies before or after it, but the old summary is
excluded from both the ordinary overview and the retained tail. The newly generated
summary supersedes it. Keeping both would duplicate inherited context on every compaction.
Other derived context is not removed merely because its origin is `DerivedSummary`.

## Limits and large sessions

Large-session compaction does not send an entire prefix transcript as the initial input.
The ordinary prefix uses a bounded overview/index plus exploration. This keeps the
transcript portion predictable without allowing an overview limit to erase inherited goals.

`overview_target_tokens` is a packing target, `overview_warning_tokens` emits a warning,
and `overview_deadline_tokens` switches the ordinary overview to a coarse index. These
limits and `compact.overview_tokens` accounting cover the overview only, not the separate
full inherited summary. Small targets, warnings, and coarse fallback must not shorten or
omit that summary.

The inherited summary still occupies the compactor's context. It counts against
`worker_context_max_tokens`; moving it outside the overview does not exempt it from the
hard input-occupancy check. If mandatory input exceeds that cap, compaction fails explicitly
before sending the oversized request, leaving the source Segment active rather than
silently clipping source material or activating an incomplete replacement. The budget guard
is installed through the internal Worker's normal `WorkerInterceptor`, not an Engine
interceptor that Worker startup would replace. Before a provider usage measurement exists,
the estimate includes the full input, system prompt, and installed tool definitions. Later
provider input measurements include that request shape already. A budget warning is an
append-only history item and is also checked against the hard cap; if it cannot fit alongside
the mandatory input, the run fails explicitly. Provider context-window failures likewise do
not activate a replacement; estimates are not an exact model-token guarantee.

`summary_target_tokens` guides the new `write_summary` output. A nonzero
`summary_max_tokens` is a hard output bound: an oversized result receives a rewrite request;
if the rewritten output still exceeds the maximum, compaction fails without activation.
This output bound is distinct from overview packing, the compactor input cap, and the
advisory `result_context_max_tokens` warning on the assembled replacement.

## Assessment boundary

Deterministic integration tests inspect the actual `LlmClient::Request` to prove full
inherited-summary transport, separation from overview packing, provenance-aware retention,
shared history-restoration behavior, hard-limit rejection without activation, and inheritance
through at least three chained compactions. Their `write_summary` outputs are predetermined
mock responses: they do **not** prove that a model understood or semantically retained user
goals, directives, or completion conditions. Shared restore-path coverage is not a complete
cold Runtime restart assessment.

Live semantic assessment remains pending. It should use a real model on long, repeated
compactions and compare the resulting summaries against the original request and explicit
completion conditions, including facts near the end of a long inherited summary and facts
absent from recent progress. Report semantic omissions separately from successful transport;
a mock echo or a successful activation cannot establish semantic continuity.

## History integrity

Compaction should preserve persisted reasoning history and avoid serializing unverified hidden reasoning context. Trace and metrics can count request shape and reasoning items without smuggling hidden provider state into model input.

The important property is explainability: after compaction, records should still show what summary replaced which older context and why future turns can rely on it.

## Binary tool details

Durable binary attachments are not text and must not be charged by applying the text
`bytes / 4` fallback to their base64 persistence encoding. Provider usage measurements remain
authoritative for requests that actually include an attachment.

An image returned by `ViewImage` is sent for the immediate follow-up model request. Later requests
project the consumed image body out while retaining the tool summary, so the Worker can call
`ViewImage` again when it needs the current file. Compaction likewise never carries image bodies
into the replacement Segment: retained images become related-file references with their Workdir
alias and path where available.

The compactor may auto-read only UTF-8 text with `mark_read_required`. Images and other binary
files can be nominated only with `add_reference`; the next Worker fetches them on demand with
`ViewImage` or the appropriate read tool.

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

Submit and Notify acceptance take the same append barrier. The controller persists
requests received while manual compaction is running immediately rather than holding
volatile deferred methods, so an overlapping Shutdown cannot discard a SubWorker
completion. Consequently an accepted item is either durably included in the
replacement checkpoint, or waits until every live authority points at the replacement
and is then appended there. Restore admission takes the allocation lock, so it cannot
observe the interval between the metadata CAS and allocation hand-off.

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
