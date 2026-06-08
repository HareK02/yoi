---
id: 20260530-053721-tui-inflight-composer-injection
slug: tui-inflight-composer-injection
title: Support immediate in-flight TUI composer injection
status: open
kind: feature
priority: P2
labels: [tui, worker, interrupt, ux]
workflow_state: planning
created_at: 2026-05-30T05:37:21Z
updated_at: 2026-05-30T05:38:11Z
assignee: null
legacy_ticket: null
---

## Background

The TUI currently lets the user press Enter while a Pod is executing, but that input is queued for the next turn. This is useful when the user wants to continue the task after the current run finishes.

There is a separate UX need: while the model is in the middle of a long run with tool calls, the user may want to send urgent supplemental context that should be seen as soon as possible, ideally between tool calls / LLM calls during the current run. This is different from ordinary queued input.

We want both modes:

- **after-run queue**: “when this task finishes, continue with this next request.”
- **in-flight injection**: “while you are still working, please incorporate this additional context as soon as safe.”

This ticket is for designing and implementing an explicit TUI path for the second mode without breaking the existing queued-input behavior.

## Requirements

- Preserve the current Enter-while-running behavior as the after-run queue.
- Add an explicit user action / keybinding / command for immediate in-flight injection while a run is active.
- In-flight injected text must be delivered through the Pod/Worker history path, not as hidden context-only injection. It must satisfy the project principle that new input placed into LLM context is first appended to `worker.history` / persisted history.
- In-flight injection should be consumed at safe boundaries, such as before the next LLM request or between tool-call cycles, not by mutating an already-open provider stream.
- The UI must make the distinction visible: queued-for-next-turn vs injected-into-current-run.
- If no run is active, the immediate-injection action should either behave like normal submit or clearly report that there is no in-flight run to inject into.
- If the current turn cannot accept in-flight input at a safe boundary, the UI should fail closed or fall back to explicit queued mode with a visible notice; do not silently drop input.
- Preserve TUI-local input history behavior for submitted/queued text.

## Non-goals

- Do not interrupt/cancel the current run as part of this ticket.
- Do not mutate provider streams already in progress.
- Do not introduce hidden system-reminder/context-only messages that are not recorded in history.
- Do not remove the existing queued composer behavior.
- Do not redesign the entire Pod notification/input protocol unless a small typed Method/Event extension is required.

## Open design questions

- What should the TUI action be?
  - Separate command such as `:inject`?
  - Modified Enter keybinding such as Ctrl+Enter / Alt+Enter?
  - Action menu entry?
- What Pod protocol shape is best?
  - Existing `Method::Notify` may already represent in-flight user-visible context, but semantics must be checked.
  - A new typed method such as `Method::InjectInput` may be clearer if `Notify` is too generic.
- What history item should represent the injected text?
  - User item?
  - System item with user-originated note?
  - Existing Notify / PodEvent item?
- What exact safe boundaries are supported in `Worker` / controller today?
  - before the next LLM request;
  - before resuming after tool results;
  - while a tool call is running;
  - while provider stream is open.
- How should the UI display pending in-flight injection versus after-run queue?

## Acceptance criteria

- TUI users can choose between after-run queued submit and immediate in-flight injection while a Pod is running.
- In-flight injected input is recorded in history before it can influence an LLM request.
- In-flight injection is consumed only at safe boundaries and never mutates an active provider stream.
- The TUI visibly distinguishes queued-next-turn input from injected-current-run input.
- Existing queued Enter behavior remains intact.
- Tests cover TUI input routing, protocol/controller handling, worker history append behavior, and safe-boundary behavior.
