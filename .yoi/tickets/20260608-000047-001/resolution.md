Merged and completed the coordinated runtime workspace / Pod identity bundle.

Summary:
- Introduced explicit separation of runtime workspace root, runtime Pod identity, and Profile selector across startup/profile/client/TUI/Pod paths.
- Removed Profile-derived Pod identity fallback and preserved Profile as recipe selection only.
- Default Pod identity now derives from runtime workspace basename with generic fallback; explicit `--pod` remains authoritative.
- `--session ... --pod ...` is accepted for restore, and hidden `--session-pod-name` was removed.
- Panel Companion/Orchestrator and Ticket role naming remain explicit and distinct.

Merged branch/worktree:
- Branch: `runtime-workspace-context`
- Commits: `b6af761`, `15f54df`
- Merge commit on `develop`: `b7a533f`

Validation passed after merge:
- focused manifest/client/pod/yoi/tui tests
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/runtime-workspace-context`.
- Deleted branch `runtime-workspace-context`.