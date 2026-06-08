Completed as part of the coordinated runtime workspace / Pod identity bundle.

Summary:
- Removed Profile slug/source/registry-derived Pod identity fallback.
- Kept Profile selection as reusable recipe selection only.
- Updated startup/spawn paths so runtime Pod identity is supplied explicitly via `--pod` and is not inferred from Profile metadata.
- Removed the hidden session-specific identity split that preserved old Profile startup behavior.

Merged branch:
- `runtime-workspace-context` via merge commit `b7a533f`.

Validation and cleanup:
- Post-merge focused tests, `cargo check -q`, `cargo fmt --check`, `git diff --check`, ticket doctor, and `nix build .#yoi` passed.
- Runtime-workspace coder/reviewer Pods, worktree, and branch were cleaned up.