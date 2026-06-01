# Implementation report

Implemented workspace-local manifest override support.

## Behavior

- Normal Profile/default resolution now searches upward from the resolver workspace base for the nearest `.yoi/override.local.toml`.
- The override is parsed as a `PodManifestConfig` layer, resolved relative to its parent `.yoi/` directory, and merged after builtin defaults plus the selected Profile but before final `PodManifest` validation and snapshot serialization.
- Resolved profile provenance now records the applied override path in `manifest.profile.workspace_override`.
- Explicit `--manifest <path>` mode remains a single-file escape hatch and does not apply workspace-local overrides.
- Workspace-local overrides are rejected if they set `pod.name`, keeping Pod identity runtime-bound.

## Validation

- `cargo test -p manifest workspace_local_override -- --nocapture`
- `cargo test -p pod manifest_mode_does_not_apply_workspace_local_override -- --nocapture`
- `cargo test -p manifest -p pod`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`

All completed successfully.
