<!-- event: create author: tickets.sh at: 2026-05-29T16:30:47Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-30T04:54:02Z -->

## Plan

## Preflight implementation plan

Classification: implementation-ready.

No product/API decision is needed before coding. `ScopeSubDelegated` remains a valid typed `PodEvent` for registry side effects and upward propagation, but must not enter the agent-visible notification/history/auto-kick lane.

Current code map:
- `crates/protocol/src/lib.rs`: `PodEvent` variants and protocol roundtrip tests.
- `crates/pod/src/controller.rs`: idle `Method::PodEvent` handler applies side effects, pushes notification, and auto-kicks; running-path handler applies side effects and pushes into `NotifyBuffer`.
- `crates/pod/src/ipc/event.rs`: transport helpers and `apply_event_side_effects`; `ScopeSubDelegated` registers grandchild and re-emits upward.
- `crates/pod/src/ipc/notify_buffer.rs`: agent-visible notification/history lane.
- `crates/pod/src/ipc/interceptor.rs`: drains `NotifyBuffer` into session history/context.
- Existing tests: controller tests for visible `TurnEnded`, pod events tests for `ScopeSubDelegated` registry/re-emission, protocol roundtrips.

Implementation phases:
1. Add `PodEvent::should_notify_agent()` classification in `protocol`: true for `TurnEnded`, `Errored`, `ShutDown`; false for `ScopeSubDelegated`.
2. Gate idle-path notification/auto-kick in `controller.rs`: always apply side effects; only push notify and schedule `RunForNotification` when `should_notify_agent()` is true.
3. Gate running-path notification buffering the same way.
4. Update comments/docs in protocol/controller/notify/event modules to distinguish control-plane side effects from agent-visible notifications.
5. Add focused tests.

Critical risks:
- Never skip `apply_event_side_effects` for `ScopeSubDelegated`.
- Gate both idle and running receive paths.
- Do not change wire serialization or remove the event.
- Do not demote `ShutDown`; it remains agent-visible.
- Do not use rendering availability as the visibility decision.

Validation plan:
- Protocol test for `should_notify_agent` classification.
- Controller test: idle `ScopeSubDelegated` updates side effects as needed but creates no `SystemItem::PodEvent`, no auto-started LLM request, and parent remains idle.
- Keep/verify existing positive `TurnEnded` auto-kick test.
- Existing `pod_events_test` should still pass for registry/re-emission.
- Run `cargo test -p protocol pod_event`, `cargo test -p pod --test pod_events`, focused controller pod-event tests, and `cargo fmt --check`.


---

<!-- event: review author: hare at: 2026-05-30T05:03:44Z status: approve -->

## Review: approve

Approve.

The implementation keeps `PodEvent::ScopeSubDelegated` on the typed IPC/control plane while removing it from the agent-visible notification/history/auto-run lane. The core change is an explicit `PodEvent::should_notify_agent()` classifier used by both controller event receive paths after side effects have already been applied.

Blocker findings: none.

Requirement coverage:
- `ScopeSubDelegated` side effects are still applied in both idle and running paths.
- Upward re-emission remains in `apply_event_side_effects`.
- `ScopeSubDelegated` no longer enters `NotifyBuffer`, does not append `SystemItem::PodEvent`, and does not auto-kick `RunForNotification`.
- `TurnEnded`, `Errored`, and `ShutDown` remain agent-visible.
- Wire serialization/protocol shape is unchanged.
- No new hidden history/context injection path was introduced.

Non-blocking follow-ups:
- Consider making misuse harder later by renaming/gating lower-level `push_pod_event_notify` / `NotifyBuffer::push_pod_event` APIs or adding debug assertions.
- Idle-path test does not directly assert registry side effect, but running-path and pod event side-effect tests cover it and the idle path calls the same side-effect function before gating.

Validation reviewed from coder report:
- `cargo fmt --check` — passed.
- `cargo test -p protocol pod_event` — passed.
- `cargo test -p pod --test pod_events_test` — passed.
- `cargo test -p pod --test controller_test pod_event` — passed.
- focused running-path tests for control-only and visible events — passed.

Final verdict: approve.


---

<!-- event: close author: hare at: 2026-05-30T05:04:26Z status: closed -->

## Closed

---
id: 20260529-163047-pod-event-scope-subdelegation-control-only
slug: pod-event-scope-subdelegation-control-only
title: Keep scope sub-delegation PodEvent out of agent notifications
status: closed
kind: bug
priority: P2
labels: [pod, events, orchestration, context]
created_at: 2026-05-29T16:30:47Z
updated_at: 2026-05-30T05:04:26Z
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


---
