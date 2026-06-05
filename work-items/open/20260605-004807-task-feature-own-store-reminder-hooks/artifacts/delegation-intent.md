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
