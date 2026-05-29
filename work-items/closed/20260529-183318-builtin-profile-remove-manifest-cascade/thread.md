<!-- event: create author: tickets.sh at: 2026-05-29T18:33:18Z -->

## Created

Created by tickets.sh create.

---

<!-- event: review author: insomnia at: 2026-05-29T19:37:45Z status: approve -->

## Review: approve

Reviewed merged implementation from branch `work/builtin-profile-remove-manifest-cascade` (`625730c`, follow-up `20ac0c9`, merged as `merge: builtin profile default startup`).

Approved after blocking fixes:

- `insomnia-pod --overlay` is no longer accepted as a user-facing generic TOML layer; SpawnPod now uses hidden typed `--spawn-config-json` and TUI restore uses typed `--session-pod-name` / `--resume-scope-json`.
- `insomnia-pod --pod <name>` fresh-create compatibility is explicit: absent Pod metadata resolves the default profile and applies the requested pod name as a typed override, with test coverage.
- TUI fresh spawn no longer has `cascade_has_scope`, `CwdDefault`, or cwd write-scope injection. Scope comes from the selected profile; session restore passes only the persisted scope snapshot.
- `resources/nix/profiles/default.nix` matches the current dogfooding manifest values and builtin profile resolution resolves `target = "."` against the launch workspace/current directory rather than the bundled profile directory.
- Profile and one-file manifest paths share `PodManifestConfig::builtin_defaults()` plus `PodManifest::try_from(...)` validation, and docs now describe prompt-loader limitations without reviving ambient manifest discovery.

Validation run in the implementation worktree:

- `cargo fmt --check`
- `cargo test -p manifest profile -- --nocapture`
- `cargo test -p pod profile -- --nocapture`
- `cargo test -p client spawn -- --nocapture`
- `cargo test -p tui spawn -- --nocapture`
- `nix eval --json --file resources/nix/profiles/default.nix >/dev/null`
- `cargo test -p pod --bin insomnia-pod -- --nocapture`
- `cargo test -p pod spawn_config -- --nocapture`
- `cargo test -p manifest -p pod -p tui --lib --bins`
- `./tickets.sh doctor`
- `git diff --check`

Known unrelated full integration failures in `cargo test -p manifest -p pod -p tui` remain in existing pod rollback integration tests and were not introduced by this ticket.


---

<!-- event: close author: hare at: 2026-05-29T19:38:49Z status: closed -->

## Closed

---
id: 20260529-183318-builtin-profile-remove-manifest-cascade
slug: builtin-profile-remove-manifest-cascade
title: Add builtin Nix profile and remove manifest cascade mode
status: closed
kind: feature
priority: P1
labels: [profiles, manifest, nix, config]
created_at: 2026-05-29T18:33:18Z
updated_at: 2026-05-29T19:38:49Z
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


---
