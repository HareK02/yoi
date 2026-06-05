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