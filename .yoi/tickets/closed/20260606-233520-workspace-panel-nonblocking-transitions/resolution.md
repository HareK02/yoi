Merged and completed the workspace panel non-blocking transition work.

Summary:
- Initial `yoi panel` now constructs a minimal/loading panel promptly and defers full snapshot plus Orchestrator/Companion ensure work into an enter-time background reload.
- Returning from nested Pod no longer awaits synchronous panel reload before the panel can be visible again; it marks refreshing and schedules an observe reload.
- Panel-originated open/attach/restore flows show visible progress before entering slow nested paths.
- Companion send, Ticket Intake launch, and Ticket action dispatch provide immediate feedback and refresh afterward without changing durable authority semantics.
- Local `PendingReload` / refreshing state guards background reload behavior and preserves selection/composer semantics as reviewed.

Merged branch/worktree:
- Branch: `workspace-panel-nonblocking-transitions`
- Commit: `12a4f39 tui: make panel transitions nonblocking`
- Merge commit on `develop`: `86198d8 merge: workspace panel nonblocking transitions`

Validation passed after merge:
- `cargo test -q -p tui multi_`
- `cargo test -q -p tui workspace_panel`
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/workspace-panel-nonblocking-transitions`.
- Deleted branch `workspace-panel-nonblocking-transitions`.

Residual risk:
- Behavior is validated with focused/unit tests rather than E2E terminal timing tests, consistent with the current project E2E gap.