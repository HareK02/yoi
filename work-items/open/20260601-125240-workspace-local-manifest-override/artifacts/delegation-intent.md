# Delegation intent: workspace-local manifest override

Intent:
- Implement a workspace-local manifest override file that overlays Profile-derived normal startup configuration.

Requirements:
- Support `.yoi/override.local.toml` as the local ignored override file.
- Detect the nearest workspace override relative to the active workspace/project base used for profile discovery.
- Apply the override as a `PodManifestConfig` layer after Profile resolution and before final `PodManifest` validation/snapshot persistence.
- Preserve explicit `--manifest <path>` as a one-file escape hatch; do not apply workspace overrides in explicit manifest mode unless you stop and report a strong reason.
- Resolve relative paths in the override file from `.yoi/` / the override file parent directory.
- Add provenance/diagnostic information where the existing manifest/profile resolution surfaces can carry it without broad refactoring.
- Add focused tests for discovery, merge order, path base, and explicit-manifest behavior.
- Update docs only if the behavior is durable and useful to document.

Invariants:
- Do not reintroduce ambient `.yoi/manifest.toml` cascade as normal startup.
- Do not put runtime-bound fields into reusable Profiles.
- Keep explicit manifest mode low-level and predictable.
- Do not read ignored secret-like file contents.
- Do not edit unrelated tickets or parent workspace files.

Non-goals:
- Do not support multiple cascading override files unless the existing workspace discovery naturally requires it.
- Do not implement a new profile language.
- Do not change dependency versions.
- Do not close the ticket or merge the worktree.

Escalate if:
- The correct workspace base is ambiguous between TUI launch, `yoi pod`, and `SpawnPod`.
- Provenance support requires changing persisted manifest schema in a broad way.
- Supporting `.yoi/override.toml` as trackable state seems necessary.
- Tests require real spawned process E2E coverage.

Validation:
- Run focused manifest/profile tests, preferably `cargo test -p manifest` plus any touched `pod` tests.
- Run `./tickets.sh doctor`, `git diff --check`, and `nix build .#yoi` if feasible in the worktree.
- Record any skipped validation with rationale.
