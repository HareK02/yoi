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

<!-- event: plan author: hare at: 2026-06-05T01:01:20Z -->

## Plan

# Delegation intent: Hook context SystemItem append sink

## Intent

Implement the first step in the Task state/reminder cleanup sequence: add event-specific Hook context support with a host-mediated durable `SystemItem` append handle, while keeping Hook return actions as per-hook-point flow-control actions.

This is the prerequisite for moving TaskStore/reminder logic into the Task feature. Do not move TaskStore or Task reminders in this ticket.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/hook-context-system-item-sink`
- branch: `work/hook-context-system-item-sink`

## Requirements

- Evolve the public Pod Hook API so Hook handlers receive event-specific context values rather than only bare summary inputs where necessary.
  - Preserve the existing per-hook-point action/output types.
  - Do not collapse actions into one global `HookDecision`.
- Add a host-created typed handle for durable model-visible `SystemItem` append.
  - Suggested naming: `SystemItemAppendHandle`, `SystemItemSink`, or equivalent.
  - Constructors/fields must stay host-private; feature/hook code can only use handles the host provides.
  - The handle must not expose raw `llm_worker::Item`, raw history writers, raw event senders, raw `Pod`, raw `Worker`, or `NotifyBuffer`.
- The append path must use existing durable history semantics:
  - host-controlled pending append / commit path;
  - `LogEntry::SystemItem`;
  - `Event::SystemItem`;
  - model context visibility only after durable commit.
- The initial approved system-item requests should be narrow.
  - Support what is needed for a future `TaskReminder` hook, and notification-like system items only if this falls out naturally from existing `SystemItem` machinery.
  - Do not introduce arbitrary `llm_worker::Item` append or generic plugin event channels.
- Keep built-in internal modules distinct from external-plugin authority approval.
  - It is okay to add scaffolding so internal hooks can receive the handle by host policy.
  - Do not implement external-plugin approval or WASM imports in this ticket.
- Preserve current observable behavior.
  - Current Task reminders should continue to be emitted by existing `PodInterceptor` logic until the follow-up ticket moves them.
  - Existing Hook tests and permission hook behavior must continue to pass.

## Non-goals

- Moving `TaskStore` ownership into the Task feature.
- Moving `TaskReminderState` or `PodInterceptor` reminder logic.
- TUI UI/dialog work.
- Generic event channel / arbitrary UI payloads.
- External plugin loading or package approval.
- Changing ToolRegistry / PreToolCall permission behavior.
- Reintroducing raw `Item` injection, `ContinueWith(Vec<Item>)`, no-result tool skip, or arbitrary `ToolResult` construction.

## Suggested files

- `crates/pod/src/hook.rs`
- `crates/pod/src/ipc/interceptor.rs`
- `crates/pod/src/pod.rs`
- `crates/pod/src/controller.rs`
- `crates/session-store/src/system_item.rs` or wherever `SystemItem` request/serialization is defined
- existing Hook tests in `crates/pod/src/**`

## Validation

Run at least:

- focused Hook context / SystemItem sink tests added by this ticket
- `cargo test -p pod hook --lib`
- `cargo test -p pod --lib`
- `cargo test -p llm-worker --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `./tickets.sh doctor`
- `git diff --check`

Run `nix build .#yoi` if feasible.

## Escalate if

- Implementing the handle requires changing session/history commit semantics.
- The only easy path is to expose raw `Item`, raw history writers, raw event senders, or `Pod`/`Worker` internals.
- Hook action types would need to be merged into one generic return type.
- Current Task reminder behavior would change before the Task ownership follow-up.

## Completion report

Report:

- worktree path / branch
- commit hash
- changed files
- Hook context/action API changes
- SystemItem append handle design and path to durable commit
- tests added/updated
- validation results
- unresolved risks/follow-ups
- whether ready for external review


---
