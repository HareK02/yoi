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
