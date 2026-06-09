# Review: workspace-local manifest override

Reviewer Pod: `workspace-override-reviewer-20260601`

## Result

Approved. No blockers found.

## Findings

The implementation satisfies the intent:

- `.yoi/override.local.toml` is discovered as the workspace-local override.
- The override applies only to normal Profile/default resolution.
- Explicit `--manifest <path>` remains a single-file escape hatch and does not load the workspace override.
- Merge order is builtin/default + selected Profile, then workspace override, then final `PodManifest` validation.
- Relative paths in the override are resolved from the override file parent `.yoi/` directory.
- `.yoi/manifest.toml` cascade was not reintroduced.
- Overrides that set `pod.name` are rejected.
- Provenance is recorded narrowly through `manifest.profile.workspace_override`.

## Follow-up handled

Reviewer requested non-blocking coverage for the case where both parent and nested `.yoi/override.local.toml` exist. Coder added commit `8f98785 test: cover nearest workspace override`, which verifies the nearest nested override wins and checks provenance.

## Validation evidence

Coder reported:

- `cargo test -p manifest workspace_local_override -- --nocapture`
- `cargo test -p pod manifest_mode_does_not_apply_workspace_local_override -- --nocapture`
- `cargo test -p manifest -p pod`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`

Parent/orchestrator reran after merge:

- `cargo test -p manifest workspace_local_override -- --nocapture`
- `cargo test -p pod manifest_mode_does_not_apply_workspace_local_override -- --nocapture`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`
- `./result/bin/yoi pod --help`

## Residual risk

`ProfileResolver::resolve()` still discovers named/default profiles from process cwd while `with_workspace_base(...)` controls scope and override discovery. This was judged non-blocking for the intended CLI/Profile/default/SpawnPod paths, but future callers should avoid assuming `with_workspace_base` also binds registry discovery.
