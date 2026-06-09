Merged and completed `SpawnPod.cwd` support.

Summary:
- Added optional `SpawnPodInput.cwd` for child process/tool working directory.
- Validates `cwd` before launch: absolute, existing directory, and usable under the child effective direct scope.
- Preserves omitted-`cwd` behavior.
- Separates runtime workspace context from tool cwd: child runtime receives inherited `--workspace`, while requested tool cwd is passed separately as hidden `--tool-cwd`.
- `Pod` now separates `workspace_root` from `pwd`; workspace/project/Ticket/workflow/memory/Profile context uses `workspace_root`, while tools/Bash/ScopedFs use `pwd`.
- Updated maintained multi-agent/worktree guidance to use `SpawnPod.cwd` as non-authority convenience while keeping delegated scope explicit.

Merged branch/worktree:
- Branch: `allow-spawnpod-child-workspace-cwd`
- Commits: `3dd7707`, `248744f`
- Merge commit on `develop`: `05df656 merge: allow SpawnPod child cwd`

Validation passed after merge:
- `cargo test -p pod spawn_pod --test spawn_pod_test`
- `cargo test -p pod spawn_pod`
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/allow-spawnpod-child-workspace-cwd`.
- Deleted branch `allow-spawnpod-child-workspace-cwd`.

Residual notes:
- Manual callers using a worktree cwd must still delegate readable workspace context plus explicit worktree scope; `cwd` grants no authority.
- Restore behavior for already-spawned Pods with distinct tool cwd was not deeply audited beyond launch-time/nested SpawnPod behavior and remains a possible future refinement if needed.