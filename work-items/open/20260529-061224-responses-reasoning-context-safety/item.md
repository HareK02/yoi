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

1. Responses reasoning items, including `encrypted_content`, must stay visible to request-shape diagnostics and documented accounting. Upstream Codex preserves reasoning items as normal API messages; Insomnia should not invent turn-boundary filtering or unverified `reasoning.context` request fields without API confirmation.
2. Pod request-threshold safety checks appear to use persisted usage history and can miss in-flight usage records from earlier LLM calls in the same run, so a long tool loop can keep issuing requests based on stale token occupancy.

## Requirements

- Reconcile `docs/ref/model-reasoning-context.md` with `crates/llm-worker/src/llm_client/scheme/openai_responses/request.rs`.
  - Document that persisted Responses reasoning items remain normal API messages unless an API-confirmed policy says otherwise.
  - Preserve the provider requirements for tool/function-call continuity.
  - Do not introduce unverified client-side reasoning filtering or request fields as a context-safety mechanism.
- Reconcile Insomnia model metadata/config semantics with upstream Codex's `context_window` / `max_context_window` split.
  - Support or document a backend max-window clamp so a user-visible 1M configured window cannot mask an effective backend limit such as 272k.
  - Ensure TUI displayed context window, compact thresholds, and request safety checks all use consistent effective-window semantics.
- Keep request construction aligned with the documented reasoning policy.
  - Add focused tests covering old reasoning items, function-call adjacency, encrypted reasoning content, and the absence of unverified `reasoning.context` serialization.
- Update Pod context safety accounting so request-threshold / pre-request checks include in-flight `UsageTracker` records from the current run, not only persisted session-log usage history.
  - Ensure long same-run tool loops can trigger compact/prune/stop decisions using the latest successful usage before the next request is sent.
- Preserve the existing principle that `Usage.input_tokens` is request prompt occupancy, while acknowledging failed `context_length_exceeded` responses may not include usage.
- Improve diagnostics for context overflow and near-overflow cases.
  - Record at least items count, item JSON bytes, raw/wire request bytes, reasoning item count, reasoning encrypted-content bytes, and whether provider usage was absent.
  - Keep diagnostics out of model context unless they are intentionally logged as normal visible events.

## Implementation notes

- Upstream Codex references for comparison:
  - `/home/hare/ghq/github.com/openai/codex/codex-rs/models-manager/models.json` defines `gpt-5.5` with `context_window=272000` and `max_context_window=272000`.
  - `codex-rs/models-manager/src/model_info.rs` clamps configured `model_context_window` by `max_context_window` when applying config overrides.
  - `codex-rs/protocol/src/openai_models.rs` derives `auto_compact_token_limit()` from the resolved context window.
  - `codex-rs/core/src/context_manager/history.rs` tracks `server_reasoning_included` and uses encrypted reasoning estimates only when the server usage does not already include them.
- Do not blindly port Codex internals. Preserve Insomnia's existing manifest/model layering and session-log authority; add the smallest typed concepts needed to represent an effective backend max window and to make safety accounting conservative enough.
- If exact reasoning inclusion policy is ambiguous, keep the request builder policy explicit in code and tests, and update `docs/ref/model-reasoning-context.md` alongside the implementation. Do not add provider request fields that are not confirmed by local schema/upstream references.
- Treat provider `context_length_exceeded` responses with `usage=null` as expected; diagnostics must rely on request-shape counters rather than nonexistent failed-request token usage.

## Acceptance criteria

- Persisted Responses reasoning items, including old `encrypted_content`, remain normal API messages unless an API-confirmed policy says otherwise.
- Function/tool-call continuity still works for Responses models that require adjacent reasoning/function-call state.
- Request safety checks include current-run in-flight usage before sending subsequent LLM calls.
- A focused regression test covers a single run with multiple LLM calls where later calls would exceed the threshold if in-flight usage were ignored.
- Focused regression tests cover a history containing old reasoning items, function-call continuity, encrypted reasoning diagnostics, and the absence of unverified `reasoning.context` serialization.
- Context overflow diagnostics make it clear when provider usage is absent and expose request-size/reasoning-size counters.
- `cargo fmt --check`
- Relevant `cargo test` / `cargo check` for `llm-worker` and `pod` pass.
