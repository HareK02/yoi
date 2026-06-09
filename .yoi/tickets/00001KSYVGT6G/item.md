---
title: "Manifest: remove filesystem resource_dir dependency"
state: "closed"
created_at: "2026-05-31T11:08:18Z"
updated_at: "2026-05-31T11:58:28Z"
---

## Background

`manifest::paths::resource_dir()` and `INSOMNIA_RESOURCE_DIR` currently keep a filesystem resource lookup boundary for bundled assets. That boundary appears increasingly historical:

- prompts are already embedded through `include_dir!` in the Pod prompt loader;
- provider/model builtin catalogs are already embedded in the provider catalog path;
- the remaining important uses are builtin Lua profile discovery and a manifest-profile model-context lookup that still reads `resources/models/builtin.toml` through `resource_dir()`.

The desired direction is to remove the filesystem `resource_dir` dependency rather than preserve `INSOMNIA_RESOURCE_DIR` as a public/user-facing configuration surface.

## Requirements

- Investigate and then remove the need for `manifest::paths::resource_dir()` if feasible.
- Remove `INSOMNIA_RESOURCE_DIR` as an active supported environment variable once no production code needs it.
- Preserve current builtin behavior:
  - builtin/default profiles remain available through normal profile selection;
  - builtin provider/model metadata remains available;
  - prompts and profile assets still resolve correctly in dev and installed/Nix builds.
- Prefer embedding builtin assets in Rust over runtime filesystem discovery where this does not create a worse design.
- If builtin Lua profiles need synthetic source labels or special handling for diagnostics/local requires, design that explicitly rather than keeping `resource_dir` by inertia.
- Update Nix packaging only after confirming resources no longer need to be installed at `share/insomnia/resources` for runtime behavior.
- Update `docs/environment.md` to remove `resource_dir` / `INSOMNIA_RESOURCE_DIR` from supported path keys if implementation removes it.

## Non-goals

- Changing user/project `config_dir` override semantics.
- Removing user profile files or project profile support.
- Changing profile selector semantics except where needed to replace builtin profile filesystem discovery.
- Reworking credential env handling.
- Reintroducing ambient manifest discovery.

## Acceptance criteria

- No production code depends on `manifest::paths::resource_dir()` for bundled asset lookup, or any remaining dependency is explicitly justified and documented in the ticket thread.
- `INSOMNIA_RESOURCE_DIR` is removed from active code/docs if `resource_dir()` is removed.
- Builtin default profile/profile registry tests pass without filesystem resource discovery.
- Builtin model/provider lookup behavior remains available and tested.
- Nix build/install checks still pass; installed package does not need runtime filesystem resources unless explicitly justified.
- `cargo fmt --check`, focused manifest/provider/pod/tui tests as relevant, `cargo check` for affected crates, `./tickets.sh doctor`, and `git diff --check` pass.
