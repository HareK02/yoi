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

<!-- event: plan author: hare at: 2026-06-05T01:27:27Z -->

## Plan

# Delegation intent: Task feature owns TaskStore and reminders

## Intent

Implement the second step in the Task feature cleanup sequence: move Task-specific state and reminder behavior out of `Pod` / `PodInterceptor` and into the built-in Task feature module.

The prerequisite `hook-context-system-item-sink` is closed. Use its `PreRequestContext` / `SystemItemAppendHandle` path for durable `SystemItem::TaskReminder` append. Pod should provide generic host surfaces; Task feature should own Task state and policy.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/task-feature-own-store-reminder-hooks`
- branch: `work/task-feature-own-store-reminder-hooks`

## Requirements

- Move TaskStore construction/ownership from `Pod` into the built-in Task feature module.
  - The Task feature should own the session-lifetime `tools::TaskStore` shared by Task tools and reminder hooks.
  - Pod should not keep a Task-specific store field merely because tools/reminders need it.
- Move TaskReminderState and reminder decision logic out of `PodInterceptor` and into Task feature-owned hooks.
  - Use a tool hook to record Task tool usage.
  - Use a `PreLlmRequest` hook to evaluate inactivity/cooldown and append `SystemItem::TaskReminder` through `SystemItemAppendHandle`.
  - Preserve the current threshold/cooldown/body/source semantics.
- Remove Task-specific checks from `PodInterceptor`, including task-tool-name special-casing for reminder state, once feature-owned hooks replace them.
- Preserve current observable behavior:
  - TaskCreate / TaskUpdate / TaskGet / TaskList names, schemas, descriptions, outputs;
  - TaskStore snapshot/restore behavior;
  - task reminder emission timing/body/cooldown;
  - model-visible history path via `LogEntry::SystemItem` / `Event::SystemItem`;
  - normal ToolRegistry / PreToolCall permission path.
- Audit all current `Pod::task_store` / `task_store` uses.
  - If Pod/session restore/compaction/TUI compatibility needs read access, route it through a Task feature-owned status/snapshot surface or a documented temporary façade that does not make Pod the owner.
  - Do not silently drop restore/snapshot/compaction behavior.
- Keep external-plugin authority model out of this ticket except using the trusted built-in hook handle path already implemented.
- Keep TUI UI changes out of scope.

## Important constraints

- Do not expose raw `llm_worker::Item`, raw history writers, raw event senders, raw `Pod`, raw `Worker`, or raw `NotifyBuffer` through the Task feature.
- Do not reintroduce raw `ContinueWith(Vec<Item>)`, no-result tool skip, arbitrary `ToolResult` construction, generic event channels, or UI/dialog payloads.
- Do not change external plugin loading, package approval, WASM, MCP, WorkItem, Memory, or Pod-management modules.
- Do not remove the existing Task tools or change their model-visible metadata.

## Suggested files

- `crates/pod/src/feature/builtin/task.rs`
- `crates/pod/src/feature.rs`
- `crates/pod/src/hook.rs`
- `crates/pod/src/ipc/interceptor.rs`
- `crates/pod/src/pod.rs`
- `crates/pod/src/controller.rs`
- `crates/tools/src/task.rs`
- any tests around TaskStore snapshot/restore and Task reminders

## Validation

Run at least:

- focused Task feature/reminder tests added or moved by this ticket
- `cargo test -p pod hook --lib`
- `cargo test -p pod --lib`
- `cargo test -p llm-worker --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `./tickets.sh doctor`
- `git diff --check`

Run `nix build .#yoi` if feasible.

## Escalate if

- Preserving TaskStore snapshot/restore requires TUI/protocol changes.
- Removing Pod ownership would require a broad feature-service/status API beyond this ticket.
- Current reminder semantics cannot be preserved through Hook order or SystemItem append timing.
- You find any hidden dependency that requires TaskStore to remain Pod-owned.

## Completion report

Report:

- worktree path / branch
- commit hash
- changed files
- where TaskStore is now owned
- how Task reminder state/logic moved into Task feature hooks
- how snapshot/restore/compaction behavior is preserved
- evidence Pod/PodInterceptor no longer own or special-case Task state
- tests/validation results
- unresolved risks/follow-ups
- whether ready for external review


---

<!-- event: review author: hare at: 2026-06-05T02:00:46Z status: request_changes -->

## Review: request changes

# External review: task-feature-own-store-reminder-hooks

## 1. Result: request changes

Request changes. The implementation substantially moves Task state and reminder policy into the built-in Task feature, but it changes the request-accounting/compaction semantics for the reminder append path: a hook-appended `SystemItem::TaskReminder` is added after the pre-request token/usage accounting has already observed the request context.

## 2. Summary of implementation

The commit introduces `TaskFeature` as the owner of the session-lifetime `tools::TaskStore` and `TaskReminderState` in `crates/pod/src/feature/builtin/task.rs`. The feature contributes the four Task tools plus two hooks:

- a `PreToolCall` hook that records `TaskCreate` / `TaskUpdate` usage;
- a `PreLlmRequest` hook that checks active tasks, inactivity threshold, cooldown, and appends a typed Task reminder through `SystemItemAppendHandle`.

`Pod` now keeps a `task_feature: TaskFeature` compatibility/status façade for restore, rewind, and compaction snapshot needs, and `PodInterceptor` no longer owns `TaskStore` / `TaskReminderState` or special-cases Task tool names in `pending_history_appends()` / `pre_tool_call()`.

## 3. Requirement-by-requirement assessment

- **TaskStore owned by Task feature/module, not Pod:** Mostly satisfied. `TaskStore` is stored inside `TaskFeatureState`; `Pod` holds a `TaskFeature` façade, not a direct `TaskStore`.
- **TaskReminderState and reminder decision logic owned by Task feature hooks:** Satisfied in structure. The threshold/cooldown/body decision logic moved to `TaskReminderPreRequestHook`; Task tool usage tracking moved to `TaskReminderToolUsageHook`.
- **PodInterceptor no longer special-cases Task tool names or emits reminders from `pending_history_appends`:** Satisfied. Current `PodInterceptor` only drains notifications in `pending_history_appends()` and dispatches generic hooks in `pre_tool_call()`.
- **Task tools use one shared session-lifetime store and preserve names/schemas/descriptions/outputs:** Appears satisfied. The feature registers the unchanged `tools::task_tools(self.state.task_store.clone())`, preserving the tool factories and shared store handle.
- **Task reminder timing/body/cooldown/source semantics match previous behavior:** Body/source/counter rules are covered, but timing/accounting is not fully preserved; see blocker below.
- **`SystemItem::TaskReminder` appended through `SystemItemAppendHandle`:** Satisfied. The Task hook uses `input.system_items().append_task_reminder(...)`; it does not construct raw `Item`s.
- **Snapshot/restore/rewind/compaction behavior preserved:** Partially satisfied. Task snapshot/overview access routes through `TaskFeature`, and rewind uses `restore_from_history()` to mutate the existing store handle so installed tools do not become stale. However, request-time compaction/accounting behavior around fired reminders is changed; see blocker.
- **Rewind/restore do not leave installed Task tool instances pointing at stale stores:** Satisfied by `TaskStore::replace_with()` through `TaskFeature::restore_from_history()`, and there is a focused unit test for this.
- **Pod does not retain Task-specific ownership under another name:** Acceptable. The `task_feature` field is a Task-specific façade in `Pod`, but the business state is inside the feature module and the façade is used for restore/compaction compatibility.
- **Normal ToolRegistry / PreToolCall permission path preserved:** Appears preserved. Task tools are still registered in the Worker tool registry, and Task usage tracking is a normal pre-tool hook after existing hook ordering rather than a bypass.
- **No unrelated TUI/plugin/event/raw-handle/refactor scope creep:** No TUI changes or broad unrelated refactors found. One boundary note remains about raw history snapshots in the feature façade; see follow-ups.
- **Tests cover ownership/reminder behavior sufficiently:** Unit coverage is good for feature-owned reminder state/body/source/cooldown and store-handle restore. It does not cover the integration point where hook-appended reminders interact with usage tracking and compaction accounting.

## 4. Blockers

### Blocker: hook-appended Task reminders are not included in pre-request usage/compaction accounting

The old Task reminder path appended the reminder from `PodInterceptor::pending_history_appends()`. `llm-worker` drains `pending_history_appends()` into persistent history before cloning `request_context`, so the reminder participated in the subsequent pre-request hooks, usage tracking, and request-threshold compaction check.

The new path appends the reminder from a `PreLlmRequest` hook:

- `TaskReminderPreRequestHook` queues the reminder through `SystemItemAppendHandle` in `crates/pod/src/feature/builtin/task.rs:204-231`.
- `PodInterceptor::pre_llm_request()` computes `current_tokens` and constructs `PreRequestInfo { item_count: context.len(), ... }` before running hooks, then drains hook system items only after all hooks finish (`crates/pod/src/ipc/interceptor.rs:212-269`).
- `UsageTrackingHook` records exactly that pre-append `item_count` (`crates/pod/src/pod.rs:224-227`).
- `llm-worker` only extends `request_context` with `PreRequestAction::ContinueWith(items)` after `pre_llm_request()` returns (`crates/llm-worker/src/worker.rs:1170-1191`).

So when the Task reminder fires, the actual LLM request includes one more model-visible history item than the usage tracker recorded for that request. The request-threshold compaction check also ran before that item existed in `request_context`. This changes the previous observable accounting/compaction behavior and can skew future token estimates and compaction timing from the first reminder-fired request onward.

This needs to be fixed before merge. The fix should preserve the host-mediated `SystemItemAppendHandle` boundary, but the final request context and the recorded usage `history_len` must include the queued system item before the LLM call is accounted/sent, matching the old `pending_history_appends()` semantics.

## 5. Non-blockers / follow-ups

- `TaskFeature::from_history()` and `TaskFeature::restore_from_history()` still take raw `&[llm_worker::Item]`. This is a narrow internal restore façade rather than a mutable raw history handle, so I am not blocking on it here, but it is worth tightening later if the feature boundary is meant to avoid raw history representations entirely.
- Add an integration-style test that installs the Task feature into the normal Pod/Interceptor path, fires a reminder, and asserts the request accounting/usage record uses the post-append context length.

## 6. Validation assessed or rerun

Reviewed:

- ticket and delegation intent;
- prerequisite hook-context-system-item-sink ticket and investigation artifact;
- `git diff develop...HEAD` for commit `c9cb2edc7e2b7d494bd20a245c0503fc91e58420`;
- relevant current files in the implementation worktree.

Commands rerun from `/home/hare/Projects/yoi/.worktree/task-feature-own-store-reminder-hooks`:

- `cargo test -p pod task_reminder --lib` — passed (8 tests).
- `cargo test -p pod task_management_tool_call_resets_reminder_inactivity_counter --lib` — passed.
- `cargo test -p pod restore_from_history_keeps_existing_store_handle_for_installed_tools --lib` — passed.
- `cargo test -p pod pre_llm_request_commits_hook_system_items_before_continue_with --lib` — passed.
- `cargo test -p pod hook --lib` — passed (14 tests).
- `git diff --check develop...HEAD` — no whitespace errors reported.

I did not rerun full `cargo test -p pod --lib`, workspace check, or `nix build .#yoi` as part of this external review.

## 7. Residual risk

After the blocker is fixed, the main residual risk is hook ordering around built-in Task hooks versus other pre-request hooks. The current design keeps hook append side effects queued and committed by the host, which is the right authority boundary, but tests should lock down where those queued items become visible for usage tracking, compaction thresholds, and the final LLM request.

---
