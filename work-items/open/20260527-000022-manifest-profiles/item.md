---
id: 20260527-000022-manifest-profiles
slug: manifest-profiles
title: Nix profile entrypoints that resolve to portable Pod manifests
status: open
kind: feature
priority: P2
labels: [manifest, profiles, nix, tui]
created_at: 2026-05-27T00:00:22Z
updated_at: 2026-05-29T16:52:47Z
assignee: null
legacy_ticket: null
---

## Migration reference

- legacy_ticket: null
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# Nix profile entrypoints that resolve to portable Pod manifests

## Background

This work item was migrated from an unfinished TODO.md entry:

> 事前定義したManifestをProfile的に扱い、Orchestrator/Coder/Researcherで別々のモデル/設定を使わせる運用ができるようにする

The current manifest cascade is good at configuration defaults by location: built-in defaults, user manifest, workspace manifest, and explicit overlays. That is less suitable for operational role selection. Users want to choose between profiles such as Orchestrator, Coder, Researcher, Reviewer, or cheap/fast variants, and they want those profiles to be portable as a pure artifact rather than assembled implicitly from several ambient layers.

Another problem is authoring ergonomics. The current manifest exposes many low-level numeric parameters that require implementation-specific intuition, such as compaction thresholds, pruning protection sizes, memory thresholds, and feature-specific token limits. Profiles should let users express high-level intent and reusable presets while the resolver produces the precise runtime manifest.

## Related work

- `work-items/open/20260529-145355-manifest-profile-encrypted-secrets/item.md`: profiles should integrate with explicit encrypted secret references so API keys/tokens are not limited to process environment variables.

## Design direction

Use Nix as the default human-authored profile format. A profile is a Nix expression that produces the final Pod manifest/configuration artifact through an Insomnia-provided `mkProfile` / `mkManifest` style library.

The profile itself is the source of truth. Commonality, imports, role presets, and any cascade-like behavior should be expressed in Nix by the profile author instead of being implemented as an additional ambient manifest cascade in Insomnia.

The runtime boundary should be:

```text
selected Nix profile + explicit startup inputs
  => deterministic resolved manifest/config snapshot
  => Pod runtime
```

Do not introduce a three-layer authoring model where Nix generates TOML profiles that then merge into TOML manifests. That would make manifest/profile/Nix ownership unclear and hard to operate. Rust should consume the resolved artifact, ideally as a typed JSON/config representation, and preserve a snapshot for Pod restore.

## Requirements

- Add a Nix-based profile entrypoint as the default path for new Pod creation.
  - Provide an Insomnia Nix library with `mkProfile` / `mkManifest` helpers.
  - The helper should produce a pure resolved manifest/config artifact that Rust can deserialize and validate.
  - Profile authors may use Nix imports/functions to share common settings, implement their own cascade, or build role presets.
- Treat the resolved manifest/config as the runtime contract.
  - Persist the selected profile identity/source and the resolved snapshot in Pod/session metadata.
  - Pod resume should prefer the saved resolved snapshot, not silently re-evaluate the Nix profile.
  - Re-evaluating a profile for an existing Pod must be explicit because it may change model, tools, permissions, or thresholds.
- Move role-oriented authoring into profiles.
  - Support profiles for roles such as Orchestrator, Coder, Researcher, Reviewer, and cost/performance variants.
  - Profiles should be able to select model/provider settings, prompts, tools, permissions, memory behavior, web/search behavior, workflows, skills, and context/compaction strategy.
  - Prefer semantic presets in the Nix library for values that are difficult to tune by raw numbers, e.g. context budget, compaction behavior, retention, autonomy, and tool policy.
  - Keep raw low-level numeric overrides available as an advanced escape hatch, not the primary user-facing interface.
- Shrink ambient cascade to discovery/default selection rather than runtime config merging.
  - User/project configuration may provide profile registries, aliases, defaults, and UI preferences.
  - User/project configuration should not be required as intermediate runtime override layers for model IDs, compaction thresholds, or other behavior controlled by the selected profile.
  - Existing TOML manifest cascade can remain as compatibility/debug/test infrastructure, but it should not be the main profile design.
- Add profile discovery and selection UX.
  - New Pod creation UI should show a selectable profile field such as `profile: coder (default)`.
  - The profile picker should list built-in/user/project/explicit profiles with enough source/default information to avoid ambiguity.
  - CLI/TUI should support explicit profile selection by name/source and by path/flakeref where appropriate.
  - Ambiguous profile names should fail closed or require source-qualified selection rather than being implicitly merged.
- Keep secrets as references, not plaintext values.
  - Nix profiles may refer to credentials using typed secret references, e.g. `secrets.ref "brave.search.default"`.
  - Nix evaluation output, resolved config serialization, diagnostics, session logs, and model context must not contain plaintext secrets.
  - Secret dereferencing/decryption happens in Rust at the consumer boundary.
- Define compatibility and fallback behavior.
  - `--manifest` / TOML manifest loading may continue to work for compatibility, tests, fixtures, and low-level debugging.
  - If Nix is unavailable, diagnostics should clearly say that profile resolution requires Nix and point to the manifest/resolved-config fallback path.
  - Existing manifest behavior should not be broken until the Nix profile path is implemented and documented.

## Open design points

- Exact Nix entrypoint shape:
  - flake output names, e.g. `insomniaProfiles.<name>` / `profiles.<name>`
  - path-based profiles, e.g. `.insomnia/profiles/coder/profile.nix`
  - whether both are supported initially
- Exact Rust-facing artifact:
  - JSON resolved config vs TOML manifest snapshot vs a new typed `ResolvedPodConfig`
  - whether `PodManifest` remains the final runtime type or becomes the legacy/compatibility representation
- Profile registry/default storage:
  - where user-level profile aliases live
  - where project-level defaults live
  - how built-in profiles are exposed
- How much Nix support is external-command based initially vs embedded/library-integrated later.
- How profile summaries are generated for the new Pod UI without exposing low-level internals or secrets.

## Acceptance criteria

- A Nix profile can be selected when creating a new Pod and resolves to the complete runtime manifest/config for that Pod.
- Insomnia provides a documented `mkProfile` / `mkManifest` Nix helper for producing a valid resolved profile artifact.
- Profile authors can share common settings and implement cascade-like composition in Nix without relying on ambient user/project manifest merging.
- New Pod UI includes profile selection and displays the effective default, e.g. `profile: coder (default)`.
- CLI/TUI profile selection supports at least one explicit path/flakeref flow and one discovered-name/default flow.
- Resolved profile artifacts are validated with clear diagnostics before Pod creation.
- Pod/session metadata persists the selected profile identity/source and the resolved snapshot.
- Pod resume uses the persisted resolved snapshot unless the user explicitly asks to reload/re-resolve the profile.
- Secret references are preserved as references through Nix evaluation and resolved config; plaintext secrets are not written to config snapshots, logs, diagnostics, or model context.
- Existing TOML manifest path remains available as a compatibility/debug/test path during the migration.
- Documentation explains the new profile model, why ambient cascade is no longer the primary runtime config mechanism, and how users should structure reusable Nix profiles.
- Focused tests cover Nix profile resolution, validation errors, profile default/source selection, ambiguity handling, snapshot persistence, and no-plaintext secret serialization paths.
- `cargo fmt --check`
- Relevant manifest/profile/pod/tui tests pass.
