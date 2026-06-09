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
