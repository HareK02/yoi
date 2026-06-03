# Implementation report: reasoning block lifecycle

## Investigation

Initial implementation unified reasoning persistence through `BlockStop.reasoning`, but OpenAI Responses text-bearing reasoning items still had two Thinking lifecycles:

1. `response.reasoning_text.delta` streamed through the real reasoning content-part block.
2. `response.content_part.done` stopped that block with no persistence metadata.
3. `response.output_item.done` emitted a second synthetic metadata-only Thinking `BlockStart`/`BlockStop` pair.

That preserved persistence but changed live callback semantics: UI/trace consumers that listen to Thinking block stop callbacks could observe an extra empty Thinking stop after the real streamed reasoning block.

## Fix summary

OpenAI Responses now defers the stop for reasoning `content_part.done` when the part is a Thinking/reasoning-text content block. At `response.output_item.done`, the provider finalizes the deferred existing block with `ReasoningBlockData` instead of creating a second synthetic live-visible block.

Thinking block handler scopes are also keyed by block index, so a deferred reasoning-text stop still uses its original streamed buffer even if another Thinking block (for example a reasoning summary block) starts and stops before `output_item.done`.

Metadata-only reasoning items with no reasoning content-part still emit a synthetic metadata-bearing Thinking block so encrypted/id-only reasoning can be persisted and round-tripped.

The fix preserves:

- live `reasoning_text.delta` Thinking deltas;
- OpenAI Responses `id`, `summary`, and `encrypted_content` persistence;
- a single Thinking lifecycle for text-bearing reasoning items;
- metadata-only reasoning coverage.

## Validation

Passed:

- `cargo test -p llm-worker openai_responses::events::tests::reasoning --lib`
- `cargo test -p llm-worker --lib`
- `cargo check --workspace --all-targets`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`

## Residual risk

The provider delays the stop event for OpenAI Responses reasoning text blocks until `response.output_item.done` so final encrypted/summary metadata can be attached to the same block. This avoids duplicate live stops but means the block stop is slightly later than the raw `content_part.done` SSE for reasoning text. This is intentional for the unified persistence model and covered by focused provider tests for the reviewed sequence.
