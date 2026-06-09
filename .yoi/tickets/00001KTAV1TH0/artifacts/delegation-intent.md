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
