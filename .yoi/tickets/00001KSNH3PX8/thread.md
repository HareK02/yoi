<!-- event: create author: tickets.sh at: 2026-05-27T20:13:13Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: orchestrator at: 2026-05-27T20:13:30Z -->

## Plan

## Background

OpenAI Responses parser currently drops unsupported SSE event types by falling through to `_ => Ok(Vec::new())`. That means provider events that do not yet have a match arm are neither visible in stream trace nor preserved as diagnostics. This made it impossible to inspect the "unexpected event" class of logs after the fact.

Recent work preserved diagnostics for known error-like event types (`response.failed`, `response.incomplete`, top-level `error`), but it did not cover event types that are not matched at all. We need observability for those raw/unhandled SSE frames without turning them into conversation history or model-visible content.

## Requirements

- OpenAI Responses SSE event types that are not otherwise handled must be observable.
  - Do not silently return `Ok(Vec::new())` without any traceable signal.
  - Include the raw `event_type` and a bounded preview of `data`.
  - Include full data length so truncation is visible.
- The signal must be visible in existing stream trace when `[session].record_event_trace = true`.
- The signal must not become assistant/user history and must not be sent back to the model as normal content.
- Timeline / collectors must ignore the signal for generation semantics.
- Known intentionally ignorable events may be classified separately if needed, but they must still be observable enough for debugging.
- Add tests for at least one unknown OpenAI Responses event type.
  - Existing `unknown_event_is_ignored` should be replaced or updated.
  - Verify event type and data preview are retained.
  - Verify large data is bounded / marked by length.

## Suggested implementation shape

A small normalized event variant is acceptable, for example:

```rust
Event::UnhandledSse {
    provider: "openai_responses",
    event_type: String,
    data_preview: String,
    data_len: usize,
}
```

or equivalent. If adding a generic variant to `llm_client::event::Event`, make sure Timeline ignores it and trace serialization captures it.

Avoid plumbing raw SSE into session history. This is observability only.

## Acceptance criteria

- Unknown OpenAI Responses SSE event types appear in trace output instead of disappearing.
- Timeline semantics / assistant output are unchanged for unknown events.
- Large raw data is capped in the event payload but original byte length is recorded.
- Focused tests pass for OpenAI Responses parser and Timeline behavior if touched.
- `cargo fmt --check` and related crate tests pass.

## Out of scope

- Implementing semantics for every OpenAI Responses event type.
- Retrying or changing behavior based on unknown events.
- Raw SSE frame permanent audit log separate from trace.


---

<!-- event: close author: hare at: 2026-05-27T20:44:19Z status: closed -->

## Closed

---
id: 20260527-201313-openai-responses-unhandled-sse-observability
slug: openai-responses-unhandled-sse-observability
title: OpenAI Responses 未対応 SSE event を破棄せず観測する
status: closed
kind: feature
priority: P1
labels: [llm, openai, observability, trace]
created_at: 2026-05-27T20:13:13Z
updated_at: 2026-05-27T20:44:19Z
assignee: null
legacy_ticket: null
---

## Background

Created by tickets.sh.

## Acceptance criteria

- TBD


---
