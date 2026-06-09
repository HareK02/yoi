---
title: "Support workspace-local manifest override layer"
state: "closed"
created_at: "2026-06-01T12:52:40Z"
updated_at: "2026-06-01T13:19:36Z"
---

## Background

Workspace profiles are already discoverable through the nearest `.yoi/profiles.toml`, but local machine/workspace overrides currently require either changing profile inputs or using an explicit `--manifest` escape hatch. This makes it awkward to keep private local settings such as model choice, reasoning level, web/search enablement, or temporary scope adjustments out of reusable Profiles.

Add a workspace-local manifest override layer that is detected from the workspace `.yoi` directory and applied after Profile resolution. The intended file name is `.yoi/override.local.toml`; `.yoi/override.toml` can be considered only if it is intentionally trackable and the semantics are documented clearly.

## Requirements

- Detect the nearest workspace-local override file while resolving a Profile/default startup path.
- Apply the override as a `PodManifestConfig` layer on top of the resolved Profile-generated manifest, before final `PodManifest` validation/snapshot persistence.
- Preserve the existing explicit `--manifest <path>` low-level escape hatch semantics; do not silently merge the workspace override into explicit manifest mode unless the implementation intentionally documents and tests that behavior.
- Resolve relative paths in the override file from the override file's parent directory.
- Record override provenance in the resolved manifest snapshot or diagnostics so local behavior is explainable.
- Prefer `.yoi/override.local.toml` as ignored/local state. If `.yoi/override.toml` is supported, define precedence and intended tracking policy.
- Do not reintroduce ambient `manifest.toml` cascade as the normal startup layer.
- Add focused tests for discovery, merge order, path base, and explicit-manifest behavior.
- Update design/development docs only where this is a durable configuration boundary.

## Acceptance criteria

- A workspace `.yoi/override.local.toml` can override Profile-derived manifest settings during normal Profile/default startup.
- Override merge order is documented and tested: builtin/default/user/project Profile resolution first, workspace-local override second, final validation last.
- `--manifest <path>` behavior is tested and either remains override-free or has explicitly documented override semantics.
- `.gitignore` / docs clarify whether `.yoi/override.local.toml` is local-only.
- Validation includes relevant manifest/profile tests, `cargo test -p manifest` or narrower equivalent, `./tickets.sh doctor`, `git diff --check`, and `nix build .#yoi` unless intentionally deferred with rationale.
