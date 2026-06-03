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
