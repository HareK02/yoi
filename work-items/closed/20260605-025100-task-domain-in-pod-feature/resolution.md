Task domain consolidation into the Pod built-in Task feature is complete and merged.

Implementation commits:

- `5469335 refactor: move task feature into pod`
- `c2ed71a docs: update tui task snapshot comments`
- merge commit on `develop`: see `merge: move task domain into pod feature`

Summary:

- Moved Task domain types and store into `pod::feature::builtin::task`.
- Moved Task tool implementations into the Pod Task feature module.
- Removed production Task APIs from `tools`, including `tools::TaskStore`, Task type re-exports, `tools::task_tools`, and Task-bearing `builtin_tools(...)`.
- Kept non-Task `tools::core_builtin_tools(...)` and tracker/non-Task tool behavior intact.
- Preserved Task tool schema/output behavior, Task snapshot/replay semantics, reminder hooks/state, and snapshot/restore/rewind/compaction façade.
- Updated TUI compatibility tests to use local fixtures without adding a production dependency on `pod` or `tools`.

Review:

- External sibling reviewer approved with no blockers.
- Reviewer non-blocker about stale TUI comments was fixed before merge.
- Remaining accepted residual risk: local TUI fixtures can drift if Pod-owned Task snapshot format changes.

Post-merge validation passed on main workspace:

- `cargo test -p pod task --lib`
- `cargo test -p tools --lib`
- `cargo test -p tools --tests`
- `cargo test -p tui task --lib`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `cargo check --workspace --all-targets`
- search for remaining production `tools` Task APIs returned no matches
