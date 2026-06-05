# Implementation report: task-domain-in-pod-feature

## Worktree / branch

- Worktree: `/home/hare/Projects/yoi/.worktree/task-domain-in-pod-feature`
- Branch: `work/task-domain-in-pod-feature`

## Commits

- `5469335 refactor: move task feature into pod`
- `c2ed71a docs: update tui task snapshot comments`

## Summary

Task domain state and Task tool implementations were moved out of the `tools` crate and into the Pod built-in Task feature module.

Final Task module layout:

- `crates/pod/src/feature/builtin/task/mod.rs`
  - Task feature contribution, reminder hook/state, snapshot/restore/rewind/compaction façade.
- `crates/pod/src/feature/builtin/task/store.rs`
  - `TaskStore`, `TaskEntry`, `TaskStatus`, `TaskSnapshot`, replay/snapshot parsing/rendering.
- `crates/pod/src/feature/builtin/task/tool_impl.rs`
  - `TaskCreate`, `TaskUpdate`, `TaskGet`, `TaskList` tool implementations and local `task_tools` factory.

`tools` now retains only non-Task core built-in tool plumbing. Production `tools::TaskStore`, `tools::TaskEntry`, `tools::TaskStatus`, `tools::TaskSnapshot`, `tools::task_tools`, and `tools::builtin_tools(..., task_store, ...)` were removed.

TUI task compatibility tests were changed to use local fixture JSON/text rather than depending on `tools` Task types. A follow-up cleanup commit updated stale comments in `crates/tui/src/task.rs` after review.

## Changed files

- `Cargo.lock`
- `package.nix`
- `crates/pod/src/feature/builtin/task/mod.rs`
- `crates/pod/src/feature/builtin/task/store.rs`
- `crates/pod/src/feature/builtin/task/tool_impl.rs`
- `crates/pod/src/ipc/interceptor.rs`
- `crates/tools/src/lib.rs`
- `crates/tools/src/tracker.rs`
- `crates/tools/tests/edge_cases.rs`
- `crates/tools/tests/integration.rs`
- `crates/tui/Cargo.toml`
- `crates/tui/src/task.rs`

## Evidence

Search for remaining production `tools` Task APIs in the implementation worktree returned no matches:

```text
rg "tools::(TaskStore|TaskEntry|TaskStatus|TaskSnapshot|task_tools|task::)|tools::task" crates -g '*.rs'
```

The implementation diff includes no `.yoi/workflow` or old `.insomnia` workflow changes.

## Validation

Coder-reported validation passed:

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
- `git diff --cached --check`
- `nix build .#yoi`

Reviewer-rerun validation passed:

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

Cleanup commit validation passed:

- `cargo test -p tui task --lib`
- `cargo fmt --check`
- `git diff --check`

## Review status

External sibling reviewer approved with no blockers. The only code/comment follow-up was handled by commit `c2ed71a`.

## Unresolved risks / follow-ups

The remaining residual risk is TUI fixture drift if the Pod-owned Task snapshot format changes without updating local TUI compatibility fixtures. This is accepted for this ticket because it avoids an undesirable production dependency from TUI back to Pod or tools.
