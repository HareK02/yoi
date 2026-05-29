---
id: 20260529-183318-builtin-profile-remove-manifest-cascade
slug: builtin-profile-remove-manifest-cascade
title: Add builtin Nix profile and remove manifest cascade mode
status: open
kind: feature
priority: P1
labels: [profiles, manifest, nix, config]
created_at: 2026-05-29T18:33:18Z
updated_at: 2026-05-29T18:33:18Z
assignee: null
legacy_ticket: null
---

## Background

Manifest profiles are now the primary way to choose a Pod runtime configuration. The old ambient manifest cascade is no longer the desired model. A manifest is the resolved/low-level Pod runtime config, not an application profile selection mechanism, and it should not be implicitly assembled from user + project layers during normal startup.

The default dogfooding behavior currently expressed in this repository's `.insomnia/manifest.toml` should be converted to a bundled builtin Nix profile and registered as the default builtin profile. Users/projects can still override the selected default through `profiles.toml`, but the runtime config should come from the selected profile artifact.

The existing defaulting/required-field logic should remain shared: profile-produced artifacts and one-file manifests should both flow through `PodManifestConfig::builtin_defaults()` and `PodManifest::try_from(...)` so defaults and required-field validation are not duplicated.

## Requirements

- Add a builtin Nix profile equivalent to the current repository `.insomnia/manifest.toml` dogfooding config.
  - Register it through builtin profile discovery.
  - It should be selectable as a builtin profile and usable as the fallback default when no user/project profile default is configured.
  - Preserve current behavior as closely as possible: model/provider, scope, worker language, compaction, memory settings, skills, web/tool settings, permissions, etc.
  - Be careful with relative paths such as `.` from the current manifest. A builtin profile must not accidentally scope writes to `resources/nix/profiles`; it should resolve workspace-root-sensitive paths correctly or introduce an explicit resolver input for the startup cwd/project root.
- Remove ambient manifest cascade from normal startup.
  - Do not implicitly merge user manifest + project manifest + overlay for normal Pod/TUI creation.
  - Normal new Pod creation should choose a profile, defaulting through profile discovery.
  - User/project `profiles.toml` remains a discovery/default registry only.
- Keep one-file manifest support as an explicit compatibility/debug path.
  - `--manifest <PATH>` should load exactly that file plus builtin defaults/validation.
  - It should not read user/project manifests.
  - It should remain mutually exclusive with `--profile` and restore/attach modes as appropriate.
- Preserve shared default/required validation.
  - `PodManifestConfig::builtin_defaults()` remains the common low-level default layer.
  - `PodManifest::try_from(PodManifestConfig)` remains the common required-field/type validation boundary.
  - Profile artifact resolution and one-file manifest loading must both use this common path.
- Update TUI/default behavior.
  - Fresh-spawn UI should default to the builtin profile when no user/project profile default exists.
  - `manifest cascade` wording should be removed or changed because cascade is no longer normal behavior.
  - If an explicit one-file manifest fallback remains selectable, label it accurately.
- Update docs/tests.
  - Documentation should describe builtin profile defaulting and explicit one-file manifest usage.
  - Remove references to ambient manifest cascade as normal startup config.
  - Tests should prove normal startup/profile discovery does not read user/project manifests as a cascade.
  - Tests should prove `--manifest` is single-file only and still shares default/required validation.

## Acceptance criteria

- Builtin profile discovery lists the converted default profile.
- With no user/project `profiles.toml`, fresh Pod creation selects the builtin profile by default.
- The converted builtin profile produces a valid `PodManifest` through the same validation path as other profiles.
- Normal startup no longer reads/merges user/project `manifest.toml` files.
- `--manifest <PATH>` remains available as single-file explicit mode and does not cascade.
- Defaults and required-field errors are shared between profile and one-file manifest paths.
- TUI labels no longer describe the default opt-out as `manifest cascade`.
- Focused manifest/profile/pod/tui/client tests pass.
- `cargo fmt --check`
- Relevant checks pass.
