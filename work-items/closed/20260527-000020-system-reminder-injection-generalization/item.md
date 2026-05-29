---
id: 20260527-000020-system-reminder-injection-generalization
slug: system-reminder-injection-generalization
title: Generalize system-reminder history append lane
status: closed
kind: task
priority: P2
labels: [pod, llm-worker, history, system-reminder]
created_at: 2026-05-27T00:00:20Z
updated_at: 2026-05-29T05:05:43Z
assignee: null
legacy_ticket: null
---

## Background

`session-todo-reminder` established the first concrete `<system-reminder>...</system-reminder>` user: Task inactivity reminders are appended through `pending_history_appends` so the reminder is persisted in `worker.history` before the next LLM request. This follows the context-processing rule that new non-volatile input must be appended to history rather than injected only into request context.

The current implementation should now be generalized so future reminder producers do not each hand-roll XML tags, `SystemItem` construction, source labeling, cooldown/priority plumbing, or history-append integration.

This ticket is about making the system-reminder append lane a small typed facility. It is not about adding new reminder policies beyond existing Task reminders.

## Requirements

- Introduce a typed internal representation for pending system reminders.
  - text/body
  - source/kind, e.g. task inactivity
  - optional priority/order key if needed
  - helper that renders the body inside `<system-reminder>...</system-reminder>` exactly once
- Route reminders through the existing `Interceptor::pending_history_appends` lane.
  - The final result must still be `Item::System(SystemItem { kind: InvokeKind::SystemReminder, ... })` or equivalent current protocol type.
  - The reminder must be appended to `worker.history`; do not introduce hidden request-only context injection.
- Refactor `session-todo-reminder` to use this typed helper/facility.
  - Task reminder behavior, thresholds, cooldown, and tests should remain unchanged.
  - The helper should prevent double-wrapping if the body is already tagged, or the API should make double-wrapping impossible.
- Keep `Notify` / `PodEvent` behavior unchanged.
  - Do not merge raw notify and system reminder semantics.
  - If they share buffering mechanics, keep the public behavior and rendered tags distinct.
- Keep ordering deterministic.
  - If multiple reminder producers are added later, ordering should be explicit or stable.
  - For now, existing Task reminder order relative to Notify/PodEvent should be preserved unless there is a clear reason to change it.
- Add docs/comments near the facility explaining the rule:
  - system reminders are durable input and must be appended through history.
  - they are not transient UI notices.
  - they are not prompt-cache/context-only injections.

## Acceptance criteria

- There is a typed system-reminder helper/facility rather than ad-hoc string construction in Task reminder code.
- Task inactivity reminders still appear as `<system-reminder>...</system-reminder>` in `pending_history_appends` output.
- The helper emits `InvokeKind::SystemReminder` / current system-reminder item kind.
- Existing Task reminder tests continue to pass.
- New focused tests cover:
  - rendering wraps body once.
  - source/kind is retained or observable where appropriate.
  - Task reminder uses the helper and remains history-append based.
  - no hidden context-only injection path is introduced.
- `cargo fmt --check`
- `cargo check -p pod -p llm-worker -p session-store`
- Relevant focused tests, e.g. `cargo test -p pod reminder --no-default-features`.

## Out of scope

- Adding a second reminder policy.
- Changing Task reminder thresholds/cooldown.
- Changing Notify/PodEvent user-visible behavior.
- UI actionbar notices.
- Prompt text changes.
- Generic notification center or reminder scheduling service.
