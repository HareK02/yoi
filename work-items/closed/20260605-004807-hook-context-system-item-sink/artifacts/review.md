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
