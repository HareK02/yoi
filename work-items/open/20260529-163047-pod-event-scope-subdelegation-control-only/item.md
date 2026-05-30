---
id: 20260529-163047-pod-event-scope-subdelegation-control-only
slug: pod-event-scope-subdelegation-control-only
title: Keep scope sub-delegation PodEvent out of agent notifications
status: open
kind: bug
priority: P2
labels: [pod, events, orchestration, context]
created_at: 2026-05-29T16:30:47Z
updated_at: 2026-05-30T04:54:02Z
assignee: null
legacy_ticket: null
---

## Background

Nested Pod orchestration currently emits a visible notification when a child Pod sub-delegates scope to its own child, for example:

```text
pod `orchestrate-nix-manifest-profiles` sub-delegated scope to `manifest-profiles-audit-20260529`
```

This comes from `PodEvent::ScopeSubDelegated`. The event itself is useful as control-plane data: parent Pods need it to update spawned-child registry state, preserve delegated scope ownership, and propagate the child/grandchild relationship upward. However, it does not usually require the parent LLM to take action.

At the moment all `PodEvent` values are pushed into the notification buffer and can trigger `RunForNotification` when the receiving Pod is idle. That makes scope delegation a model-visible semantic notification, adds noise to history/context, and can cause unnecessary auto-kicked LLM turns during nested orchestration.

## Requirements

- Keep `PodEvent::ScopeSubDelegated` as a control-plane event.
  - Existing registry side effects must still run.
  - Scope ownership/reclaim behavior must not regress.
  - Upward propagation to higher-level parents must still happen when needed.
- Do not expose scope sub-delegation as an agent notification.
  - Do not push `ScopeSubDelegated` into the Pod notification buffer.
  - Do not persist it as model-visible notification history.
  - Do not trigger `PendingRun::RunForNotification` solely because scope was sub-delegated.
- Preserve agent-visible notifications for events that need orchestration attention.
  - `TurnEnded` should remain agent-visible.
  - `Errored` should remain agent-visible.
  - `ShutDown` should remain agent-visible unless a later design explicitly separates it.
- Make the event visibility boundary explicit in code.
  - Prefer a small helper such as `PodEvent::should_notify_agent()` or an equivalent visibility classification.
  - Keep side effects and agent notification decisions separate so future control-plane events do not accidentally become model-visible.
- Keep context/history principles intact.
  - Control-plane-only events must not be injected into LLM context without first becoming intentional history content.
  - Avoid extra prompt-cache churn and token use for events that are not actionable by the model.

## Suggested implementation notes

Likely areas:

- `crates/protocol/src/lib.rs`: add an explicit visibility/helper on `PodEvent`.
- `crates/pod/src/controller.rs`: after `apply_event_side_effects`, only call `pod.push_pod_event_notify(event)` and set `PendingRun::RunForNotification` when the event is agent-visible.
- `crates/pod/src/ipc/event.rs`: keep `ScopeSubDelegated` side effects unchanged.
- `crates/pod/tests/controller_test.rs`: update/add coverage for control-only scope delegation and agent-visible lifecycle events.

## Acceptance criteria

- `ScopeSubDelegated` still updates/propagates spawned-child registry state exactly as before.
- `ScopeSubDelegated` no longer produces `[Notification] ... sub-delegated scope ...` in the parent Pod's agent-visible output/history.
- `ScopeSubDelegated` does not auto-kick an idle parent Pod into a model run.
- `TurnEnded`, `Errored`, and `ShutDown` still produce agent-visible notifications and can still wake an idle parent when appropriate.
- Tests cover both the control-only `ScopeSubDelegated` path and at least one agent-visible `PodEvent` path.
- `cargo fmt --check`
- Relevant pod/protocol tests pass.
