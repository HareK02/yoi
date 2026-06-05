<!-- event: create author: tickets.sh at: 2026-06-05T02:51:00Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-05T02:53:15Z -->

## Decision

# Decision: keep built-in Task feature inside pod for now

Task is a stateful built-in feature, not a low-level `tools` crate concern. The next step should keep the feature inside `pod` rather than creating a new crate.

Rationale:

- `pod::feature` / `pod::hook` are still defined in the `pod` crate.
- Creating `builtin-features` now would either depend on `pod` or require premature extraction of a feature-api crate.
- Feature-per-crate would create too many crates; a future `builtin-features` crate may be appropriate only after the API boundary is stable.
- Moving Task domain state out of `tools` and into `pod::feature::builtin::task` fixes the immediate semantic split without forcing crate-boundary churn.

Desired result for this ticket:

- `tools` provides low-level generic tool helpers.
- `pod::feature::builtin::task` owns TaskStore, Task types, Task tool implementations, Task reminders, and Task feature lifecycle.


---

<!-- event: plan author: hare at: 2026-06-05T02:53:16Z -->

## Plan

# Delegation intent: move Task domain into pod built-in feature

## Intent

Move Task domain state and Task tool implementation out of the `tools` crate and into the `pod::feature::builtin::task` module. Keep this inside the `pod` crate for now; do not create a new built-in-features crate.

The goal is cohesion: Task is now a stateful built-in feature, so `TaskStore`, Task types, Task tools, reminder hooks, and snapshot/restore façade should live together.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/task-domain-in-pod-feature`
- branch: `work/task-domain-in-pod-feature`

## Requirements

- Move Task domain implementation from `crates/tools/src/task.rs` into the Pod built-in Task feature module.
  - If the file becomes large, prefer `crates/pod/src/feature/builtin/task/` with submodules such as `store.rs`, `tools.rs`, `reminder.rs`, and `mod.rs`.
  - A single `task.rs` file is acceptable if the change stays clear and maintainable.
- Task feature should own:
  - `TaskStore`
  - `TaskEntry`
  - `TaskStatus`
  - `TaskSnapshot`
  - `TaskCreate` / `TaskUpdate` / `TaskGet` / `TaskList` tool implementations
  - reminder hooks/state already moved to the feature
  - snapshot/restore/rewind/compaction façade
- Remove Task production API from `tools`.
  - Remove `tools::TaskStore`, `tools::TaskEntry`, `tools::TaskStatus`, `tools::TaskSnapshot`, and `tools::task_tools` re-exports unless a narrowly justified temporary test-only compatibility path is required.
  - Remove or update `tools::builtin_tools(..., task_store, ...)`; production tools should not imply Task belongs to `tools`.
  - Keep `tools::core_builtin_tools(...)` and non-Task low-level tools intact.
- Update Pod Task feature code to use local Task types and local Task tool factories.
- Move/port Task tests from `tools` to `pod` as needed.
- Update `tools` integration tests so they no longer expect Task tools in `tools::builtin_tools` registration order.
- Update TUI compatibility tests that currently depend on `tools` Task types.
  - Prefer local JSON fixtures or local mirrored structs in TUI tests.
  - Do not make TUI depend on `pod` just for tests unless there is a strong reason and it does not create an undesirable dependency.
- Preserve observable behavior exactly:
  - tool names/schemas/descriptions;
  - tool outputs;
  - TaskStore replay/snapshot text;
  - Task reminder body/cooldown/source;
  - TUI parsing compatibility;
  - normal ToolRegistry / PreToolCall path.

## Non-goals

- Creating a new `builtin-features` crate.
- Extracting `feature-api` from `pod`.
- External plugin loading, WASM, package approval, sandbox authority work.
- Moving other built-in tool groups.
- TUI UI changes.
- Changing Task semantics.

## Suggested files

- `crates/pod/src/feature/builtin/task.rs`
- `crates/pod/src/feature/builtin.rs`
- `crates/pod/src/feature.rs`
- `crates/tools/src/task.rs`
- `crates/tools/src/lib.rs`
- `crates/tools/tests/integration.rs`
- `crates/tools/tests/edge_cases.rs`
- `crates/tui/src/task.rs`
- `crates/tui/src/app.rs` / TUI task compatibility tests if needed

## Validation

Run at least:

- focused Task feature tests moved/updated by this ticket
- `cargo test -p pod task --lib`
- `cargo test -p tools --lib`
- `cargo test -p tools --tests`
- `cargo test -p tui task --lib` or relevant focused TUI task tests
- `cargo test -p pod --lib`
- `cargo test -p llm-worker --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `./tickets.sh doctor`
- `git diff --check`

Run `nix build .#yoi` if feasible.

## Escalate if

- Removing `tools::builtin_tools` breaks important non-Pod production callers.
- TUI compatibility checks require a shared Task schema crate to avoid duplication.
- Moving Task tool implementation into `pod` creates an unexpected dependency cycle.
- Preserving Task snapshot/replay semantics would require behavior changes.

## Completion report

Report:

- worktree path / branch
- commit hash
- changed files
- final Task module layout
- what remains in `tools` and why
- evidence production code no longer uses `tools::TaskStore` / `tools::task_tools`
- tests/validation results
- unresolved risks/follow-ups
- whether ready for external review


---

<!-- event: review author: hare at: 2026-06-05T03:22:31Z status: approve -->

## Review: approve

# Review: task-domain-in-pod-feature

## 1. Result

approve

## 2. Summary of implementation

The implementation relocates the Task domain implementation out of `tools` and into `pod::feature::builtin::task`. The new Task feature module now owns the task store/domain types, task tool handlers, feature installation, and the existing reminder/snapshot/restore/rewind/compaction façade. The `tools` crate is reduced to the non-Task built-in tools and no longer exposes `tools::TaskStore`, `tools::task_tools`, or a `builtin_tools(..., task_store, ...)` API.

TUI task parsing remains local to `tui` and does not add a production dependency on either `pod` or `tools`; its tests use local snapshot JSON fixtures instead of importing the previous TaskStore type.

## 3. Requirement-by-requirement assessment

- **Task domain ownership under `pod::feature::builtin::task`: satisfied.** `TaskStore`, `TaskEntry`, `TaskStatus`, `TaskSnapshot`, task tool implementations, reminder hook/state, and snapshot/restore/rewind/compaction methods are cohesive under the Pod built-in Task feature module.
- **No production Task APIs exposed from `tools`: satisfied.** The `tools` crate no longer has the Task module/API surface and `core_builtin_tools()` now installs only non-Task built-ins.
- **Non-Task `tools` behavior remains intact: satisfied.** The remaining tool modules, exports, and tests are conceptually unchanged apart from removing Task-specific test coverage from `tools`.
- **Task tool names/schemas/descriptions/outputs unchanged: satisfied.** The TaskCreate/TaskUpdate/TaskGet/TaskList definitions and handler response strings were moved without semantic changes.
- **TaskStore replay/snapshot/restore and reminder behavior unchanged: satisfied.** The store logic and TaskFeature façade preserve the previous append-history, snapshot, restore, rewind, overview, reminder, and compaction behavior.
- **TUI task compatibility without undesirable dependency: satisfied.** `tui` keeps its own typed compatibility reader and does not depend on `pod` or `tools` in production.
- **Normal ToolRegistry / PreToolCall path unchanged: satisfied.** Task tools are still registered through the built-in feature contribution path and continue through the existing Worker ToolRegistry / PreToolCall policy path.
- **No broad unrelated architecture changes: satisfied.** I did not find new crate-boundary/API extraction, plugin loading, authority-model, WorkItem/MCP, generic UI/event-channel, or broad refactor work in this diff.
- **`package.nix` / `Cargo.lock`: acceptable.** The `Cargo.lock` change follows from removing the `tools` dev-dependency from `tui`; the `cargoHash` update is therefore expected. The conditional `fetchCargoVendor` static-crates patch is not part of the Task-domain move, but it is a narrow and safe packaging guard around an existing static-crates workaround, and `nix build .#yoi --no-link` passed with it.
- **No `.yoi/workflow` or old `.insomnia` workflow changes included: satisfied.** The diff has no changed files under those paths.

## 4. Blockers

None.

## 5. Non-blockers / follow-ups

- `crates/tui/src/task.rs` still has a stale comment referring to `tools::render_snapshot` / `tools::TaskStore` ownership of the snapshot format. The code is fine, but the comment should be updated in a follow-up or before merge if the author is already revising the branch.
- TUI compatibility tests now rely on local fixture JSON, which is appropriate for avoiding a production dependency, but it leaves some residual drift risk if the Pod-owned snapshot format changes without updating the fixture.

## 6. Validation assessed or rerun

Reviewed:

- Ticket, delegation intent, and decision artifact.
- `git diff develop...HEAD` for the implementation branch.
- Changed Task feature, tools, TUI, packaging, and lockfile files.
- Search results for remaining `tools::TaskStore`, `tools::task_tools`, workflow, and `.insomnia` changes.

Reran from `/home/hare/Projects/yoi/.worktree/task-domain-in-pod-feature`:

- `cargo test -p pod task --lib`
- `cargo test -p tools --lib`
- `cargo test -p tools --tests`
- `cargo test -p tui task --lib`
- `cargo test -p pod --lib`
- `cargo test -p llm-worker --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi --no-link`

All rerun validation completed successfully.

## 7. Residual risk

The main residual risk is fixture drift in TUI task compatibility tests now that `tui` intentionally does not import the Pod-owned TaskStore type. That risk is acceptable for this ticket because the implementation preserves the existing serialized shape and avoids the undesirable production dependency.


---
