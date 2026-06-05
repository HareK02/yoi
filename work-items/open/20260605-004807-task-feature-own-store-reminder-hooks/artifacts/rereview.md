# External rereview: task-feature-own-store-reminder-hooks

## 1. Result: approve

Approve. The prior blocker is fixed: hook-appended Task reminders are now folded into the effective request context before usage accounting and the second request-threshold compaction decision, and threshold-yield preserves the queued system item durably instead of dropping model-visible history.

## 2. Summary of fix reviewed

The fix adds an internal `PreRequestAction::YieldWith(Vec<Item>)` variant in `llm-worker` and updates `PodInterceptor::pre_llm_request()` to:

1. keep the existing pre-hook request-threshold check for the already-present context;
2. run pre-request hooks and drain any host-mediated `SystemItemAppendHandle` appends;
3. build an `effective_len = context.len() + system_items.len()`;
4. run request-threshold compaction again against the post-append effective context;
5. if the post-append threshold yields, commit the queued system items and return `YieldWith(items)` so `Worker` appends them to history before yielding;
6. otherwise record usage with the effective post-append length and continue with the queued items.

This keeps the hook append boundary host-mediated while making the model-visible request context match the context used for usage tracking.

## 3. Prior-blocker assessment

- **Hook-appended `SystemItem::TaskReminder` included before usage accounting:** Fixed. `PodInterceptor::pre_llm_request()` now drains hook system items before the final `usage_tracker.note_request(...)` call and computes `effective_len` from `context.len() + system_items.len()`.
- **`UsageTracker::note_request()` uses post-append context length:** Fixed. The call now passes `effective_len`, so a fired Task reminder is counted in the recorded request history length.
- **Request-threshold compaction checks the post-append context:** Fixed. After hook drain, `estimated_tokens` is recomputed over a `Cow` that includes the queued items, and `request_threshold.should_compact(current_tokens)` runs again before the request is sent.
- **Threshold yield preserves queued system items durably:** Fixed. On post-append threshold yield, `PodInterceptor` commits the queued system items, returns `PreRequestAction::YieldWith(items)`, and `Worker` appends those items into history before returning `WorkerResult::Yielded`. The next request therefore starts from durable/in-memory history that includes the reminder.
- **`YieldWith(Vec<Item>)` does not expose public raw injection through `pod::hook`:** Acceptable. `YieldWith` is an internal `llm-worker` interceptor action, not a public `pod::hook` action. The public hook-facing append API remains the typed `SystemItemAppendHandle`; public hook types do not gain raw `Item` construction authority.

## 4. Rechecked requirements

- **TaskStore/reminder ownership remains in Task feature:** Still satisfied. `TaskStore` and `TaskReminderState` remain inside `crates/pod/src/feature/builtin/task.rs`; `Pod` keeps only the feature façade needed for installation, restore, rewind, and compaction snapshot/overview calls.
- **No `PodInterceptor` Task special-casing returned:** Confirmed. Production `PodInterceptor` code does not branch on Task tool names and does not own Task reminder state; Task behavior is driven through feature hooks.
- **Task tool metadata/behavior unchanged:** Confirmed by diff scope. The Task tool implementations and metadata remain in `crates/tools/src/task.rs`; the feature still registers `tools::task_tools(...)` with the shared feature-owned store.
- **No raw `Item` exposure in public Hook API:** Confirmed. `SystemItemAppendHandle` exposes typed append methods such as `append_task_reminder`; the raw `Item` vectors remain inside worker/interceptor internals.
- **No generic event channels or UI dialogs:** Confirmed in the reviewed diff. The fix is confined to worker/interceptor request flow and tests.
- **Snapshot/restore/rewind semantics remain safe:** Still acceptable. `TaskFeature::restore_from_history()` mutates the existing shared store handle, preserving installed Task tool handles after restore/rewind. The prior focused restore test still passes.
- **Tests cover fixed accounting/compaction path:** Satisfied. New/focused tests cover post-append usage length and post-append request-threshold yield preservation, in addition to the existing Task reminder and hook append tests.

## 5. Blockers

None.

## 6. Non-blockers / follow-ups

- `TaskFeature::from_history()` / `restore_from_history()` still accept raw `llm_worker::Item` slices as an internal restore façade. This remains acceptable for this ticket, but can be tightened in a later boundary-cleanup pass if feature restore APIs are made more typed.
- The new `YieldWith(Vec<Item>)` is an internal escape hatch with raw `Item`s. It is justified here for durable preservation of host-created system items on yield, but future uses should stay interceptor-owned and should not be surfaced through public plugin/hook APIs.

## 7. Validation assessed or rerun

Reviewed:

- original review artifact;
- ticket and delegation intent;
- `git diff develop...HEAD` and the fix commit `960f2a3 fix: account hook system item appends`;
- current `llm-worker`, `pod::ipc::interceptor`, Task feature, hook API, and relevant Pod restore/snapshot code.

Commands rerun from `/home/hare/Projects/yoi/.worktree/task-feature-own-store-reminder-hooks`:

- `cargo test -p pod task_reminder_hook_append_is_counted_in_usage_request_len --lib` — passed.
- `cargo test -p pod pre_llm_request_yields_with_hook_appends_when_post_append_threshold_exceeded --lib` — passed.
- `cargo test -p pod hook --lib` — passed.
- `cargo test -p llm-worker --lib` — passed.
- `cargo test -p pod task_reminder --lib` — passed.
- `cargo test -p pod restore_from_history_keeps_existing_store_handle_for_installed_tools --lib` — passed.
- `git diff --check develop...HEAD` — no whitespace errors reported.

I did not rerun full workspace tests or `nix build .#yoi` as part of this rereview.

## 8. Residual risk

Residual risk is low for this ticket. The main area to watch is future expansion of `YieldWith(Vec<Item>)`: it should remain an internal worker/interceptor mechanism for preserving already-authorized, host-created history appends across yield/compaction boundaries, not a public raw-history injection mechanism.