---
id: 20260529-061224-responses-reasoning-context-safety-review
slug: responses-reasoning-context-safety
title: Review for responses reasoning context safety
status: reviewed
kind: review
created_at: 2026-05-29T06:12:24Z
updated_at: 2026-05-29T07:05:00Z
reviewer: insomnia-system
---

## Review summary

Reviewed implementation branch `work/responses-reasoning-context-safety` in worktree `/home/hare/Projects/insomnia/.worktree/responses-reasoning-context-safety`.

The first review pass found two blocking issues:

1. The implementation filtered persisted Responses reasoning items by latest-user/current-turn boundaries. Upstream Codex preserves reasoning as normal API messages and handles old encrypted reasoning through accounting, so this was not justified by local evidence. The implementation was amended in `8ed5939` to preserve reasoning history.
2. The implementation added top-level Responses `reasoning.context = "current_turn"`. The local upstream Codex schema only defines `effort` and `summary` for request reasoning, so adding an unverified provider field risked request rejection. The implementation was amended in `27b1891` to remove the field and associated docs/tests.

After those amendments, the remaining implementation is acceptable for the ticket scope:

- Adds `max_context_window` metadata and clamps effective `context_window` by backend max.
- Registers builtin `gpt-5.5` with `context_window = 1000000` and `max_context_window = 272000`.
- Includes in-flight `UsageTracker` records in pre-request safety accounting.
- Adds request-shape and reasoning encrypted-content diagnostics.
- Preserves persisted reasoning history and function-call continuity.
- Updates `docs/ref/model-reasoning-context.md` to document the current policy and diagnostic/accounting approach.

## Validation

Implementation Pod reported:

- `cargo fmt --check`
- `cargo test -p provider --lib`
- `cargo test -p llm-worker --lib openai_responses`
- `cargo test -p llm-worker --lib llm_client::transport::tests::request_body_shape_counts_reasoning_encrypted_content`
- `cargo test -p pod --lib ipc::interceptor::tests`
- `cargo check --workspace`

Reviewer reran focused validation after the final amendment:

- `cargo fmt --check`
- `cargo test -p provider --lib`
- `cargo test -p llm-worker --lib openai_responses`
- `cargo test -p pod --lib ipc::interceptor::tests`

## Judgment

Approved after amendments. Merge is safe, subject to normal conflict handling with the main workspace's unrelated task reminder cadence change.
