<!-- event: create author: tickets.sh at: 2026-06-05T00:48:07Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-05T00:49:53Z -->

## Decision

# Investigation: Task state and Hook side-effect boundary

## Findings

- Hook action types are already separated per hook point after Hook hardening. The next design should preserve that: flow-control actions stay event-specific rather than becoming one global `HookDecision`.
- Hook inputs are still summary structs, not contexts with host-created handles. That is the missing abstraction for feature-owned behavior that needs durable host side effects.
- `TaskStore` is still owned by `Pod`, and `TaskReminderState`/reminder emission is still owned by `PodInterceptor`.
- The Task built-in feature module currently contributes only the four Task tools and receives `TaskStore` from Pod. This is an incomplete internal-module boundary: Task-specific state still remains on the Pod side.
- `SystemItem::TaskReminder` is currently appended through `PodInterceptor::pending_history_appends()`, which is the correct durable history direction but the wrong ownership location for Task-specific logic.

## Decision

Split follow-up into two steps:

1. Add Hook context host handles, especially a durable `SystemItem` append handle. Hook returns remain per-hook-point flow-control actions. No raw `Item` injection and no generic effect/event channel.
2. Move TaskStore and Task reminder logic into the Task feature module, implemented as Task-owned tools plus hooks that use the host-provided SystemItem append handle.

This keeps Pod responsible for generic host surfaces and history authority, while Task owns Task-specific state and policy.


---
