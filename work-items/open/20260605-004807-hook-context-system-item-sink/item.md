---
id: 20260605-004807-hook-context-system-item-sink
slug: hook-context-system-item-sink
title: Hook: add context handles for host-mediated SystemItem append
status: open
kind: feature
priority: P1
labels: [hooks, feature-registry, history, task-reminder]
created_at: 2026-06-05T00:48:07Z
updated_at: 2026-06-05T01:01:20Z
assignee: null
legacy_ticket: null
---

## Issue

Built-in feature modules need a closed way to influence Pod-side behavior without adding feature-specific logic back into `Pod` / `PodInterceptor`. Task reminders are the immediate example: a Task feature should be able to observe request/tool activity and append a `SystemItem::TaskReminder`, but it should not receive `Pod`, `Worker`, raw history writers, raw `llm_worker::Item` injection, or a generic effect channel.

The current Hook surface already has per-hook-point action types after `hook-public-surface-hardening`, and those action types should remain flow-control oriented. The missing piece is event-specific Hook context carrying host-owned handles for allowed side effects, especially a durable `SystemItem` append path.

## Current findings

- `crates/pod/src/hook.rs` already distinguishes hook point output types such as `HookPromptAction`, `HookPreRequestAction`, `HookPreToolAction`, `HookPostToolAction`, and `HookTurnEndAction`.
- `PreLlmRequest` / `OnTurnEnd` public actions no longer expose raw `Item` injection.
- Hook inputs are currently summary structs (`PreRequestInfo`, `ToolCallSummary`, etc.), not context objects with host handles.
- `TaskReminder` emission currently lives in `PodInterceptor::pending_history_appends()` and uses Pod-owned `TaskStore` / `TaskReminderState`.
- Existing durable model-visible path is `SystemItem` commit -> `LogEntry::SystemItem` -> `Event::SystemItem` / model context, and this path must remain the only model-visible append mechanism.

## Direction

Introduce event-specific Hook context objects that carry narrow, host-created handles. Keep Hook return values as per-hook-point flow-control actions.

- Side effects are not returned as arbitrary `HookEffect` enums.
- Side effects are requested through typed handles on the Hook context.
- Handles have private constructors and are created only by the Pod host.
- Missing authority/handle should prevent installation or make the handle unavailable; runtime rejection is defense-in-depth, not the normal API path.

## Requirements

- Define event-specific Hook context types, or evolve `HookEventKind::Input` into context values without losing the existing per-event action types.
  - `PreLlmRequest` context should be able to carry request summary plus a SystemItem append handle where the host grants it.
  - Tool hook contexts should continue to expose bounded call/result summaries.
- Add a host-owned `SystemItemAppendHandle` / sink concept for model-visible durable system items.
  - It must not expose raw `llm_worker::Item` or raw history writer access.
  - It should append/push only approved `SystemItem` request kinds, initially including TaskReminder/Notification-like system items as needed.
  - Actual commit timing remains host-controlled through the existing pending history append / `LogEntry::SystemItem` / `Event::SystemItem` path.
- Preserve per-hook-point flow-control action types.
  - Do not collapse every hook into one generic `HookDecision`.
  - Do not reintroduce `ContinueWith(Vec<Item>)`, no-result tool skip, arbitrary `ToolResult` construction, or hidden context mutation.
- Keep internal built-in module usage distinct from external-plugin authority approval.
  - Built-in modules may receive host handles according to host policy.
  - Future external plugins receive model-visible append handles only when the external-plugin authority model grants them.
- Provide tests showing:
  - Hook actions remain flow-control only;
  - model-visible side effects use the SystemItem append handle;
  - no raw `Item` / raw history writer / raw event sender is exposed through public Hook context;
  - missing/unavailable handles do not produce invisible context injection.

## Non-goals

- Moving TaskStore/reminder state into the Task feature; that is tracked by `task-feature-own-store-reminder-hooks`.
- TUI dialog/custom UI work.
- Generic plugin event channels.
- External plugin loading/WASM boundary implementation.
- Changing ToolRegistry / PreToolCall permission behavior.

## Acceptance criteria

- Hook contexts can carry host-owned typed handles for allowed side effects, starting with durable SystemItem append.
- Hook return values remain event-specific flow-control actions.
- Model-visible additions through Hooks are committed through the durable SystemItem path and are user-inspectable.
- Public Hook APIs still do not expose raw `Item` injection, raw Pod/Worker/session internals, or generic UI/event payload channels.
- The resulting API is sufficient for a later Task reminder Hook to append `SystemItem::TaskReminder` without Pod-specific Task logic in `PodInterceptor`.
