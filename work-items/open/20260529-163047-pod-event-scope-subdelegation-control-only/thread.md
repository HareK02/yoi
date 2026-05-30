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
