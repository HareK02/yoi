Completed as part of the coordinated runtime workspace / Pod identity bundle.

Summary:
- Replaced product-name-specific default Pod identity with runtime workspace basename-based default naming.
- Preserved explicit `--pod` precedence.
- Kept workspace Orchestrator and Ticket role/task Pod names explicit and distinct from the default Companion/workspace Pod.
- Added/validated non-`yoi` workspace/default naming coverage so the dogfooding repository name no longer masks hardcoded defaults.

Merged branch:
- `runtime-workspace-context` via merge commit `b7a533f`.

Validation and cleanup:
- Post-merge focused tests, `cargo check -q`, `cargo fmt --check`, `git diff --check`, ticket doctor, and `nix build .#yoi` passed.
- Runtime-workspace coder/reviewer Pods, worktree, and branch were cleaned up.