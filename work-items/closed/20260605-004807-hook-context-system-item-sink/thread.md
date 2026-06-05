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

<!-- event: review author: hare at: 2026-06-05T01:25:03Z status: approve -->

## Review: approve

# Review: hook-context-system-item-sink

## 1. Result: approve

Approve. I found no blockers for merging `04a4a73 pod: add hook system item sink` against the stated prerequisite scope.

## 2. Summary of implementation

The implementation changes the `PreLlmRequest` hook input from bare `PreRequestInfo` to `PreRequestContext`, while leaving hook return types event-specific and flow-control-only. `PreRequestContext` carries the existing request summary plus an optional `SystemItemAppendHandle`.

`SystemItemAppendHandle` is host-constructed (`pub(crate) fn new`), has private storage, and exposes only `append_task_reminder(...)`, which queues a typed `SystemItem::TaskReminder`. `PodInterceptor::pre_llm_request` creates a per-request queue only when a log writer exists, passes the handle through the hook context, and after all pre-request hooks continue successfully drains the queue, commits each item through the existing `SystemItemCommitter` / `LogEntry::SystemItem` path, and returns the rendered history items via the internal `PreRequestAction::ContinueWith` lane.

The existing Task reminder implementation remains in `PodInterceptor::pending_history_appends()` and Task ownership is not moved in this ticket.

## 3. Requirement-by-requirement assessment

- **Hook return actions remain per-hook-point flow-control types:** Satisfied. `HookPromptAction`, `HookPreRequestAction`, `HookPreToolAction`, `HookPostToolAction`, and `HookTurnEndAction` remain distinct. `HookPreRequestAction` still exposes only `Continue`, `Cancel`, and `Yield`; it does not expose `ContinueWith(Vec<Item>)`.
- **Public API avoids raw internals and generic effects:** Satisfied for the changed Hook surface. The new handle does not expose raw `llm_worker::Item`, history writers, event senders, `Pod`, `Worker`, `NotifyBuffer`, internal interceptor actions, no-result skip, arbitrary `ToolResult` construction, or a generic event/UI channel. Existing pre-tool denial still maps an error string to an internal synthetic result, not an arbitrary public `ToolResult`.
- **`SystemItemAppendHandle` host-private construction and narrow approved surface:** Satisfied. The constructor and backing queue are crate-private/private, and public hook code can only request a task-reminder system item through `append_task_reminder`.
- **Durable model-visible append path:** Satisfied within existing semantics. Hook-queued items are committed through the existing `SystemItemCommitter` path as `LogEntry::SystemItem`, which in production publishes through the segment log sink as `Event::SystemItem`; only then are corresponding model-visible history items returned to the worker through the internal interceptor action.
- **Successful-continue-only injection:** Satisfied by code. `pre_llm_request` returns immediately on the first non-continue hook action before draining the local queue, so queued hook items are dropped on cancel/yield and are committed/injected only when all hooks continue.
- **Task reminder behavior preserved:** Satisfied. `TaskStore`, `TaskReminderState`, `task_reminder_system_item`, `pending_history_appends`, and task-tool reset behavior remain in `PodInterceptor`; the follow-up ticket remains responsible for moving Task ownership into the Task feature.
- **Existing permission/hook behavior remains conceptually valid:** Satisfied. Tool hook contexts remain bounded summaries; public pre-tool actions still provide only continue/deny/abort/pause, with no no-result skip or arbitrary tool result path.
- **Tests:** Mostly satisfied. Added tests cover handle queueing of an approved task reminder, context handle absence, commit plus returned system message behavior, and no-log-writer behavior. Existing tests continue to cover public hook action restrictions and current task reminder lanes. The only test gap I noted is explicit append-then-cancel/yield no-leak coverage, but the implementation path is straightforward enough that I do not consider this a merge blocker.
- **No broad refactor / no external-plugin approval implementation:** Satisfied. The diff is limited to `crates/pod/src/hook.rs`, `crates/pod/src/ipc/interceptor.rs`, and a small `PreRequestContext` type update in `crates/pod/src/pod.rs`.

## 4. Blockers

None.

## 5. Non-blockers / follow-ups

- Add an explicit regression test for a pre-request hook that appends through `SystemItemAppendHandle` followed by a later hook returning `Cancel` or `Yield`, asserting that no commit and no `ContinueWith` occur. The code already behaves this way, but the intended no-leak invariant is important enough to lock down.
- Before any external plugin/user-script hook surface is enabled, add per-hook or per-feature authority policy for whether `PreRequestContext::system_items()` is present. This ticket intentionally avoids external-plugin approval, and current built-in/internal hook usage is acceptable.

## 6. Validation assessed or rerun

Assessed:

- Reviewed ticket, delegation intent, investigation note, authority-context decision, and dependent Task ownership follow-up.
- Reviewed `git diff develop...HEAD`; changed files are limited to:
  - `crates/pod/src/hook.rs`
  - `crates/pod/src/ipc/interceptor.rs`
  - `crates/pod/src/pod.rs`
- Inspected the relevant public hook API, handle construction/surface, interceptor commit path, and task reminder preservation tests.

Rerun:

- `cd /home/hare/Projects/yoi/.worktree/hook-context-system-item-sink && git diff --check develop...HEAD` — passed with no output.

Not rerun:

- I did not rerun `cargo test`, `cargo check`, or `nix build` from this external-reviewer turn because the requested validation allowance was read-only and those commands would write build artifacts. I did not find a coder validation report in the ticket thread.

## 7. Residual risk

The remaining risk is in future authority granularity, not this prerequisite implementation: today the handle is present for pre-request hooks when a log writer exists, rather than being selected per installed feature/hook. That should be tightened before exposing hooks to untrusted external plugins. The existing `SystemItemCommitter` API also logs and drops commit failures rather than returning a result; this is pre-existing behavior shared by other SystemItem lanes, but it means disk-write failure handling is not made stronger by this ticket.


---

<!-- event: close author: hare at: 2026-06-05T01:26:06Z status: closed -->

## Closed

Added PreRequestContext and host-mediated SystemItemAppendHandle for pre-request hooks. Hook returns remain per-hook-point flow-control actions; the append handle has host-private construction and a narrow task-reminder request surface; queued system items commit through the existing LogEntry::SystemItem / Event::SystemItem path only on successful continue. Existing Task reminder ownership remains unchanged for the follow-up ticket. External review approved. Merge validation passed: cargo test -p pod hook --lib, cargo test -p pod --lib, cargo test -p llm-worker --lib, cargo fmt --check, cargo check --workspace --all-targets, ./tickets.sh doctor, git diff --check, nix build .#yoi, ./result/bin/yoi pod --help.


---
