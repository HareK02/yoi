---
id: 20260529-180257-profile-registry-config-file
slug: profile-registry-config-file
title: Move profile registry out of manifest files
status: open
kind: bug
priority: P1
labels: [manifest, profiles, config]
created_at: 2026-05-29T18:02:57Z
updated_at: 2026-05-29T18:02:57Z
assignee: null
legacy_ticket: null
---

## Background

The initial Nix manifest profiles implementation made profile discovery/default/alias metadata available from `[profiles]` sections inside `manifest.toml`. That is a design error.

A manifest is the Pod runtime configuration contract: model, scope, tools, memory, web, permissions, prompts, and other settings that are already selected for a Pod. Profile registry/default/alias metadata is application/project UX configuration used before a Pod manifest exists. Putting registry metadata inside `manifest.toml` makes the manifest both the runtime config and the profile selector config, contradicting the profile design.

This was caught immediately after merge, before compatibility needs exist. Do not keep backward compatibility for `[profiles]` in manifest files.

## Requirements

- Move profile registry/default/alias configuration out of manifest files.
  - User registry file: `<config_dir>/profiles.toml`.
  - Project registry file: closest `.insomnia/profiles.toml` found by walking up from the cwd.
  - Builtin profile discovery can remain under bundled resources.
- Remove manifest-file profile registry parsing.
  - `manifest.toml` must not be the source for profile registry/default/alias data.
  - `INSOMNIA_USER_MANIFEST` must not affect profile registry discovery.
  - `PodManifestConfig` / `PodManifest` must remain runtime config only.
- Use a top-level `profiles.toml` schema rather than nesting under `[profiles]`.
  - Example:
    ```toml
    default = "coder"

    [profile]
    coder = "profiles/coder.nix"

    [alias]
    work = "coder"
    ```
  - The existing table form for entries may remain useful:
    ```toml
    [profile.coder]
    path = "profiles/coder.nix"
    description = "Project coder"
    ```
- Keep existing profile selection semantics.
  - `default`, unqualified names, source-qualified names, aliases, ambiguity handling, and default markers should continue to work.
  - Unqualified alias/default targets remain source-local by default.
  - TUI fresh-spawn profile row and manifest-cascade opt-out should continue to work.
- Update docs and tests.
  - Documentation must describe `profiles.toml`, not `[profiles]` in `manifest.toml`.
  - Tests should create `.insomnia/profiles.toml` / user `profiles.toml`, not `.insomnia/manifest.toml`, for registry behavior.
  - Add coverage proving `[profiles]` in `manifest.toml` is ignored for discovery.

## Acceptance criteria

- User profile registry is discovered from `<config_dir>/profiles.toml`.
- Project profile registry is discovered from closest `.insomnia/profiles.toml`.
- `[profiles]` in user/project `manifest.toml` is not read for profile registry/default/alias discovery.
- `INSOMNIA_USER_MANIFEST` does not change profile registry discovery.
- Existing CLI/TUI selector behavior still works with `profiles.toml`.
- Docs no longer instruct users to put profile registry metadata in `manifest.toml`.
- Focused manifest/tui/pod/client profile tests pass.
- `cargo fmt --check`
- Relevant checks pass.
