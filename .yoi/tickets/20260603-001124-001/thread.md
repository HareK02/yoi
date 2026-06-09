<!-- event: create author: tickets.sh at: 2026-06-03T00:11:24Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-03T00:51:09Z -->

## Plan

# Delegation intent: unify reasoning persistence with block lifecycle

Intent:
- Implement the `unify-reasoning-block-lifecycle` ticket by removing the separate `ReasoningItem` side channel and making the reasoning/thinking block lifecycle the single authoritative path for live streaming and persistence.

Requirements:
- Start by mapping the current reasoning paths in `llm-worker`: Anthropic thinking stream, OpenAI Responses reasoning items, Timeline block handlers, `ReasoningItemCollector`, Worker finalization, and request serialization round-trip tests.
- Remove the separate finalized reasoning event path:
  - `ReasoningItemEvent`;
  - `ReasoningItemKind`;
  - `ReasoningItemCollector`;
  - `Timeline::on_reasoning_item` / `dispatch_reasoning_item` / `reasoning_item_handlers`.
- Extend the reasoning/thinking block lifecycle so stop/finalization carries all provider material needed to build `Item::Reasoning`:
  - text;
  - reasoning item id;
  - summary;
  - encrypted content;
  - Anthropic thinking signature;
  - redacted thinking payload metadata.
- Convert Anthropic `thinking_delta` / `signature_delta` / redacted thinking handling to finalize through reasoning block stop metadata, without emitting a separate `ReasoningItem` event.
- Convert OpenAI Responses completed reasoning items into reasoning block lifecycle events, including metadata-only reasoning where there is no streamed text delta.
- Update Worker collection/finalization so `Item::Reasoning` is built from reasoning block lifecycle state.
- Preserve live streaming thinking/reasoning callbacks for UI/trace consumers.
- Preserve persisted reasoning history round-trip behavior for Anthropic and OpenAI Responses.
- Remove misleading comments that treat reasoning as meta/single-event content.
- Do not add backward compatibility aliases or keep duplicate old/new reasoning concepts.

Invariants:
- Do not drop provider material required for reasoning round-trip (`signature`, redacted thinking metadata, `id`, `summary`, `encrypted_content`).
- Do not hide model-affecting reasoning persistence in a context-only path; persisted reasoning must remain explainable through committed history items.
- Do not redesign unrelated provider request serialization.
- Do not read ignored secret-like file contents.
- Do not edit the parent workspace; work only in the delegated worktree.
- Do not close the ticket, merge the branch, delete worktrees, or push.

Non-goals:
- No E2E spawned-process test framework.
- No product-level behavior changes beyond event model cleanup.
- No dependency changes.
- No compatibility layer for the removed `ReasoningItem` event path.

Escalate if:
- A provider requires reasoning persistence material that cannot naturally fit in block lifecycle metadata.
- OpenAI Responses reasoning items cannot be represented as synthetic block lifecycle events without losing ordering or identity semantics.
- The change requires broad public API churn outside `llm-worker` and its direct consumers.
- Existing tests imply a behavior conflict between live streaming callbacks and persistence.

Validation:
- Run focused reasoning/timeline/provider tests that cover Anthropic thinking signature/redacted material and OpenAI Responses `id` / `summary` / `encrypted_content` round-trip.
- Run `cargo test -p llm-worker --lib`.
- Run `cargo check --workspace --all-targets`.
- Run `./tickets.sh doctor` and `git diff --check`.
- Run `nix build .#yoi` if feasible; record if skipped and why.
- Commit the implementation in the worktree when reviewable.

Completion report:
- investigation summary;
- implementation summary;
- changed files;
- commit hash(es);
- validation commands and results;
- unresolved risks or parent decisions needed;
- whether ready for external review.


---

<!-- event: review author: hare at: 2026-06-03T02:15:01Z status: approve -->

## Review: approve

# Review: unify reasoning block lifecycle

Reviewer Pods:

- Initial review: `reasoning-block-lifecycle-reviewer-20260603`
- Re-review: `reasoning-block-lifecycle-rereviewer-20260603`

## Result

Approved after fixes. No remaining blockers.

## Initial blocker

The initial reviewer found that OpenAI Responses text-bearing reasoning items produced two live-visible Thinking lifecycles:

1. streamed `reasoning_text.delta` used the real Thinking block;
2. later `response.output_item.done` emitted a second synthetic empty Thinking block to carry persistence metadata.

That preserved persistence but changed live callback/trace semantics beyond the intended event-model cleanup.

## Fix verification

Coder added commit `abb6adb fix: preserve openai reasoning live stops`.

The re-review confirmed:

- text-bearing OpenAI reasoning now attaches `ReasoningBlockData` to the deferred existing Thinking block stop;
- no second synthetic empty Thinking start/stop is emitted for text-bearing reasoning;
- metadata-only OpenAI reasoning still persists through a synthetic metadata-bearing Thinking block when no live reasoning text block exists;
- Anthropic signature/redacted material and OpenAI id/summary/encrypted_content remain carried through `ReasoningBlockData`;
- the old `ReasoningItem` event/collector/Timeline side channel was not reintroduced.

## Validation evidence

Coder reported:

- `cargo test -p llm-worker --lib`
- `cargo test -p llm-worker --test reasoning_round_trip_test`
- `cargo check --workspace --all-targets`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`

Parent/orchestrator reran after merge:

- `cargo test -p llm-worker openai_responses::events::tests::reasoning --lib`
- `cargo test -p llm-worker --lib`
- `cargo test -p llm-worker --test reasoning_round_trip_test`
- `cargo check --workspace --all-targets`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`
- `./result/bin/yoi pod --help`

## Residual risk

Low. OpenAI Responses reasoning-text block stop is intentionally delayed until `response.output_item.done` so provider persistence metadata can be attached to the same Thinking block lifecycle. The remaining edge risk is unusual ordering/error behavior around multiple concurrent Thinking scopes, but the implementation now keys Thinking scopes by block index and focused tests cover the reviewed duplicate-stop sequence.


---

<!-- event: close author: hare at: 2026-06-03T02:15:02Z status: closed -->

## Closed

Unified reasoning persistence with the Thinking block lifecycle. Removed the separate ReasoningItem event/collector path; Anthropic and OpenAI Responses reasoning metadata now persist via block stop metadata; reviewer blocker fixed; focused tests and nix build passed.


---
