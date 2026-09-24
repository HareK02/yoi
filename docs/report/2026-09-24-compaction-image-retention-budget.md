# Compaction retained image payloads exceed the result budget after generation

## Symptom

A dogfood Worker repeatedly failed automatic compaction with:

```text
pre-run compaction failed: compacted result context too large: 199726 tokens exceeds max 60000
automatic compaction failed and the provider request remained unsafe: result_context_too_large
```

The triggering human message was `continue`. The failure appeared only after the Compactor had spent multiple minutes generating a summary.

## Observed context shape

The retained tail ended with one parallel group of five `ViewImage` calls and results. The PNG payloads totalled approximately 589 KB. Their durable tool-result attachments contained approximately 785 KB of base64 data:

| Result                    |   PNG bytes | base64 characters |
| ------------------------- | ----------: | ----------------: |
| representative mobile     |      67,274 |            89,700 |
| partial failure narrow    |      95,730 |           127,640 |
| conflict desktop          |     144,897 |           193,196 |
| representative desktop    |     145,057 |           193,412 |
| operation loading desktop |     136,012 |           181,352 |
| **Total**                 | **588,970** |       **785,300** |

The retained split requested an 8,000-token tail. Its byte-derived target was about 32 KB, but the last indivisible tool result alone serialized to about 182 KB. Pair-boundary repair then moved the cut backward to the first parallel `ViewImage` call, retaining all five calls and results. The repaired retained tail serialized to approximately 788 KB.

The result check calls `agen::token_counter::total_tokens(&new_history, &[])`. With no result-segment usage records, this uses serialized JSON bytes divided by four. The retained image group alone therefore became an estimated 197k tokens. Summary, references, and TaskStore items raised the final estimate to 199,726 tokens.

This is not a provider token measurement. It is a fallback estimate dominated by base64 image bytes.

## Execution ordering

`compact_impl` determines the retained items before starting the Internal Compactor, but checks `result_context_max_tokens` only after the Compactor has generated its summary, completed any nudge turn, re-read nominated files, and assembled `new_history`.

The observed Worker made three deterministic failed attempts against the same source Segment:

| Trigger           |  Duration |
| ----------------- | --------: |
| request threshold | 197.757 s |
| pre-run           | 183.572 s |
| pre-run           | 155.872 s |

The final pre-run attempt finished 45 ms before the persisted `continue` input and run error. The ordinary Worker provider request was not sent. After pre-run compaction failed, the request-threshold interceptor saw the same unsafe occupancy and correctly failed closed with `result_context_too_large`.

Thus the safety stop occurs before the ordinary provider request, but after an expensive Compactor provider run whose result can never pass the current result-budget calculation.

## Design problems

1. **Image attachments are counted as text.** Serialized base64 bytes divided by four are not an appropriate estimate for provider image-input occupancy.
2. **The retained budget is soft at item and pair boundaries.** One large result can exceed the budget, and pair repair can pull an entire parallel group back into the retained tail.
3. **An impossible result is detected too late.** Retained items are known before spawning the Compactor. Under the current estimator, their lower bound already exceeded the 60k result maximum.
4. **The same deterministic failure is retried on each logical run.** Failure suppression lasts only for the current run, so repeated `continue` submissions spend another 2–3 minutes producing a result that is discarded.
5. **The post-generation error obscures what consumed the budget.** The alert reports only the final total and maximum, not retained/summary/auto-read/task breakdown or the fallback estimate source.

## Suggested direction

- Give attachment/image inputs provider-shape-aware token accounting rather than charging their base64 serialization as text.
- Before starting the Compactor, calculate and report a retained-result lower bound. Do not run the model when no legal summary can fit the result maximum.
- Make retained-tail construction capable of projecting large tool-result details/attachments while preserving the durable summary and call/result pairing, or move the entire oversized completed tool group to the summarized side.
- Persist enough failure state to avoid retrying an unchanged, deterministically impossible source Segment.
- Include bounded breakdown fields in the failure diagnostic: estimate source, retained items/tokens, attachment bytes, summary tokens, auto-read tokens, and fixed synthetic context.

The final post-compaction safety check should remain. The defect is that it uses the wrong accounting for images and is the first effective result-size check rather than the last guard after an earlier feasibility decision.

## Implemented follow-up

The follow-up change stops interpreting durable image bytes as text tokens and removes the need for compaction to carry image bodies:

- fallback token accounting projects binary attachments out before JSON byte estimation;
- an image attachment is included in the immediate follow-up request, then projected out of later request contexts while its durable tool summary remains;
- retained compaction images are stripped from the replacement Segment and emitted as related-file references, preserving `target_workdir` and path where available;
- `mark_read_required` rejects images and non-UTF-8 files, directing the compactor to `add_reference` instead.

Exact provider usage remains authoritative for the request that actually consumes an image. The post-compaction maximum remains as the final safety guard.
