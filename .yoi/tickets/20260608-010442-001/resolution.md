Merged and completed the direct/delegation authority split.

Summary:
- Added a separate `delegation_scope` alongside direct `scope` in manifest/profile resolution.
- `SpawnPod` requested child scope is now validated against delegation authority rather than direct tool scope.
- Missing old delegation metadata/snapshots default to no delegation, so restored older Pods fail closed for child delegation.
- Direct tool scope remains available for parent `Read`/`Write`/`Edit`/`Bash` behavior.
- Orchestrator profile receives explicit workspace-write delegation; base/non-orchestrator role profiles do not inherit broad delegation.
- Fixed reviewer-identified recursive/non-recursive subset and deny-overlap edge cases with path-set based validation.

Merged branch/worktree:
- Branch: `split-direct-and-delegation-authority`
- Commits: `a4a9b00`, `f43c8ac`
- Merge commit on `develop`: `92d1c0b merge: split direct and delegation authority`

Validation passed after merge:
- `cargo test -p manifest profile --lib`
- `cargo test -p manifest deserialize_old_manifest_snapshot_defaults_to_no_delegation --lib`
- `cargo test -p manifest delegation_ --lib`
- `cargo test -p manifest --lib`
- `cargo test -p pod spawn_pod --test spawn_pod_test`
- `cargo test -p pod-registry`
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/split-direct-and-delegation-authority`.
- Deleted branch `split-direct-and-delegation-authority`.

Residual notes:
- Non-recursive path-set validation is intentionally conservative and path-based; it does not infer whether a direct child path is a file or directory.
- Future child-to-grandchild subdelegation support will require an explicit child-delegation request/validation/persistence surface and remains out of scope.