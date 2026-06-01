<!-- event: create author: tickets.sh at: 2026-06-01T00:16:16Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-01T00:16:59Z -->

## Plan

## Investigation notes

- Representative session: `~/.insomnia/sessions/019e8042-be06-72e2-bc80-05afdfde4515/`.
- First segment: `019e8042-be06-72e2-bc80-05b98007803a.jsonl`.
- Compact segment: `019e8043-4d63-7231-b0c7-3d356e86665a.jsonl` with `compacted_from.at_turn_index = 1`.
- The compact path appears to be mid-turn request threshold yield, not prune itself:
  - `PodInterceptor::pre_llm_request()` checks `state.exceeds_request(current)`.
  - `PreRequestAction::Yield` becomes `WorkerResult::Yielded`.
  - `Pod::handle_worker_result()` runs `do_compact_and_resume()`.
- `prune.fire` observed in the same segment is useful context when reading the log, but prune is not the compact trigger and this ticket does not require prune behavior changes.

## Design constraint

Do not model system and history as exactly separable token domains unless the implementation can measure them as such. For compact thresholding, the stable property is whole request prompt occupancy.


---

<!-- event: plan author: hare at: 2026-06-01T00:41:18Z -->

## Plan

## Preflight classification

implementation-ready.

The ticket is a bounded bug fix in compact/request-threshold token accounting. The intended behavior is clear: compact thresholding should estimate whole request prompt occupancy and must not divide provider `input_total_tokens` by history-only bytes. Prune behavior is explicitly out of scope.

## Requirements sync

Observable completion:

- A fresh session / one prior usage record case with large fixed prompt overhead does not trigger request-threshold yield solely because history grew after the first measured request.
- Measured `input_total_tokens` remains authoritative for the exact measured request occupancy.
- Unmeasured request occupancy estimation uses a size measure that corresponds to the same whole request context being estimated, not history-only bytes.

Non-goals:

- Do not change prune behavior or prune savings policy for this ticket.
- Do not change compact thresholds or profile defaults as the fix.
- Do not alter session log schema unless the implementation finds it necessary and escalates first.

## Current code map

- `crates/llm-worker/src/token_counter.rs`: shared token estimate functions used for compact/request thresholding; current extrapolation uses history prefix bytes.
- `crates/pod/src/ipc/interceptor.rs`: `pre_llm_request` computes request-context estimate and yields when `request_threshold` is exceeded.
- `crates/pod/src/compact/token_counter.rs`: Pod-side wrappers and tests around token estimates; prune helpers exist here but are not in ticket scope.
- `crates/pod/src/compact/usage_tracker.rs`: captures usage records keyed by in-flight request history length.
- `crates/pod/src/compact/state.rs`: threshold semantics; should not need behavior changes.
- `crates/llm-worker/src/worker.rs`: request loop and prune projection before `pre_llm_request`; should not need lifecycle changes.

## Critical risks

- Fixing the estimate by simply raising thresholds or disabling request-threshold yield would hide the bug and is not acceptable.
- Splitting system/tool/history into separate exact token domains is not warranted unless the implementation can measure the same request shape consistently.
- Regression tests must exercise the one-measurement case, because that is where fixed prompt overhead previously dominated the inferred history rate.
- Reviewer should verify that prune behavior was not intentionally changed.

## Intent packet

Intent:
- Fix compact/request-threshold token occupancy estimation so whole prompt usage is not projected from history-only bytes.

Requirements:
- Treat exact usage records as authoritative for the measured request occupancy.
- Estimate unmeasured whole request occupancy using a request-size basis that corresponds to the whole request context, or a conservative fallback that does not allocate fixed prompt overhead to history bytes.
- Add regression coverage for first-turn/fresh-session overestimation.

Invariants:
- Compact remains triggered by threshold semantics, not by prune activity.
- Prune behavior is out of scope and should not be changed intentionally.
- Do not introduce a false exact system/history token split.
- Do not modify profile thresholds as the fix.

Escalate if:
- The clean fix requires session-log schema changes, provider request serialization changes, or durable migration.
- The implementation would change prune behavior or compact lifecycle semantics.

Validation:
- Focused Rust tests for `llm-worker` token counter and pod compact/interceptor behavior as applicable.
- `cargo test -p llm-worker token_counter` or narrower exact test target if available.
- `cargo test -p pod compact` or focused pod tests if touched.
- `cargo check --workspace` if focused tests pass and runtime is reasonable.
- `./tickets.sh doctor` in main workspace before finalization.


---
