---
id: 20260529-061224-responses-reasoning-context-safety
slug: responses-reasoning-context-safety
title: Fix context safety accounting for Responses reasoning
status: open
kind: bug
priority: P1
labels: [llm-worker, pod, compact, reasoning]
created_at: 2026-05-29T06:12:24Z
updated_at: 2026-05-29T06:12:24Z
assignee: null
legacy_ticket: null
---

## Background

A long-running `gpt-5.5` session hit `context_length_exceeded` while the TUI still showed roughly `190k/400k`. The failing request was in session `019e6bcf-fc62-7f93-b117-39369699c2c3`, segment `019e6e18-c777-7be0-af32-9a2585e19ff7`, `turn=1195`, `llm_call=9`.

The immediate trace showed the last successful usage event reported `input_tokens=197700`, while the failed request returned no usage. The request diagnostics also showed `reasoning.context="current_turn"` and a large request body (`items_len=2617`, `items_json_bytes=1775947`, `raw_json_bytes=1834360`, `wire_bytes=686528`). The same segment contained hundreds of persisted reasoning items with substantial `encrypted_content`.

A cross-check against `/home/hare/ghq/github.com/openai/codex` found that upstream Codex does not assume every configured context window is directly usable. Its model metadata has both `context_window` and `max_context_window`, and `ModelInfo::resolve_context_window()` clamps user `model_context_window` by `max_context_window` when present. Upstream also carries a `GPT_5_BEDROCK_CONTEXT_WINDOW = 272_000`, which matches the observed successful-session ceiling much better than the locally configured 1M window. Insomnia needs to distinguish advertised/configured window, backend max window, and compact/request thresholds.

Two implementation areas need to be corrected together so context safety checks match what the Responses backend actually receives:

1. `openai_responses` request construction appears to project persisted `Item::Reasoning` entries, including `encrypted_content`, back into the next request without enforcing the intended `reasoning.context` / current-turn / function-call adjacency policy documented in `docs/ref/model-reasoning-context.md`.
2. Pod request-threshold safety checks appear to use persisted usage history and can miss in-flight usage records from earlier LLM calls in the same run, so a long tool loop can keep issuing requests based on stale token occupancy.

## Requirements

- Reconcile `docs/ref/model-reasoning-context.md` with `crates/llm-worker/src/llm_client/scheme/openai_responses/request.rs`.
  - Define exactly which reasoning items may be sent for `reasoning.context="current_turn"`.
  - Preserve the provider requirements for tool/function-call continuity.
  - Do not silently resend old reasoning `encrypted_content` outside the documented policy.
- Reconcile Insomnia model metadata/config semantics with upstream Codex's `context_window` / `max_context_window` split.
  - Support or document a backend max-window clamp so a user-visible 1M configured window cannot mask an effective backend limit such as 272k.
  - Ensure TUI displayed context window, compact thresholds, and request safety checks all use consistent effective-window semantics.
- Update request construction so persisted reasoning items are included only when required by the documented policy.
  - Add focused tests covering old reasoning items, current-turn reasoning, function-call adjacency, and encrypted reasoning content.
- Update Pod context safety accounting so request-threshold / pre-request checks include in-flight `UsageTracker` records from the current run, not only persisted session-log usage history.
  - Ensure long same-run tool loops can trigger compact/prune/stop decisions using the latest successful usage before the next request is sent.
- Preserve the existing principle that `Usage.input_tokens` is request prompt occupancy, while acknowledging failed `context_length_exceeded` responses may not include usage.
- Improve diagnostics for context overflow and near-overflow cases.
  - Record at least items count, item JSON bytes, raw/wire request bytes, reasoning item count, reasoning encrypted-content bytes, and whether provider usage was absent.
  - Keep diagnostics out of model context unless they are intentionally logged as normal visible events.

## Acceptance criteria

- `reasoning.context="current_turn"` no longer causes old persisted reasoning `encrypted_content` to be resent outside the documented policy.
- Function/tool-call continuity still works for Responses models that require adjacent reasoning/function-call state.
- Request safety checks include current-run in-flight usage before sending subsequent LLM calls.
- A focused regression test covers a single run with multiple LLM calls where later calls would exceed the threshold if in-flight usage were ignored.
- A focused regression test covers a history containing old reasoning items and verifies request input contains only the allowed reasoning subset.
- Context overflow diagnostics make it clear when provider usage is absent and expose request-size/reasoning-size counters.
- `cargo fmt --check`
- Relevant `cargo test` / `cargo check` for `llm-worker` and `pod` pass.
