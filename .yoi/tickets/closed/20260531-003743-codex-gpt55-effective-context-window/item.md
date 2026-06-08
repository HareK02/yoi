---
id: 20260531-003743-codex-gpt55-effective-context-window
slug: codex-gpt55-effective-context-window
title: 'Provider: make codex gpt-5.5 context window effective'
status: closed
kind: task
priority: P2
labels: [provider, models, catalog]
created_at: 2026-05-31T00:37:43Z
updated_at: 2026-05-31T01:58:24Z
assignee: null
---

## Background

`refresh-builtin-model-catalog` updated the `codex-oauth/gpt-5.5` builtin entry to record OpenAI's advertised 1.05M context window while retaining `max_context_window = 272000` as an effective clamp. That made the catalog entry technically accurate to both OpenAI docs and observed Codex OAuth behavior, but it also made the model metadata harder to understand.

For a provider-specific builtin entry, `context_window` should represent the effective window for that provider route. `codex-oauth/gpt-5.5` is not the generic OpenAI API route; it is the Codex OAuth / ChatGPT backend route. If that route effectively fails above ~272k input tokens, then the builtin catalog should say `context_window = 272000` directly and explain why in a comment.

## Requirements

- Change the builtin `codex-oauth/gpt-5.5` model entry so `context_window` is the effective Codex OAuth route limit (`272000`).
- Remove `max_context_window = 272000` from that builtin entry.
- Add a nearby TOML comment explaining that OpenAI documents GPT-5.5 with a 1.05M context window, but Codex OAuth / ChatGPT backend access has been observed/known to be effectively limited around 272k, so the provider-specific builtin entry records the effective limit.
- Keep the default profile on `codex-oauth/gpt-5.5`.
- Preserve the `max_context_window` manifest/config field and its code/tests if still useful for explicit inline overrides or future providers; do not remove the field globally in this ticket unless doing so is clearly tiny and mechanically safe.
- Update provider/manifest tests that currently expect catalog-level `max_context_window` clamp for builtin `gpt-5.5`.
- Do not change OpenRouter `openai/gpt-5.5` unless there is separate evidence for that route's effective limit; this ticket is about `codex-oauth`.

## Non-goals

- Re-refreshing the full model catalog.
- Changing provider definitions or auth behavior.
- Changing compaction/pruning algorithms beyond tests that derive known model context.
- Removing `max_context_window` from public manifest/config types wholesale.

## Acceptance criteria

- `resources/models/builtin.toml` has `codex-oauth/gpt-5.5` with `context_window = 272000` and no `max_context_window` field.
- The builtin catalog comment makes the 1.05M-vs-272k route distinction explicit.
- Tests no longer rely on catalog-level `max_context_window` for `codex-oauth/gpt-5.5`.
- Inline `max_context_window` clamp behavior remains tested if the field remains supported.
- `cargo fmt --check`, focused provider/manifest tests, `./tickets.sh doctor`, and `git diff --check` pass.
