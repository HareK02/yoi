---
id: 20260529-181528-remove-profile-aliases
slug: remove-profile-aliases
title: Remove profile aliases from profile registry
status: closed
kind: bug
priority: P1
labels: [profiles, config, simplification]
created_at: 2026-05-29T18:15:28Z
updated_at: 2026-05-29T18:20:44Z
assignee: null
---

## Background

The initial profile registry added `[alias]` as a convenience layer for redirecting one profile name to another. In practice this adds name-resolution complexity without a clear use case. It already caused bugs around source-local alias resolution and defaults pointing at aliases.

Profile selection should stay simple: a registry contains profile entries and an optional default that points directly at one profile entry. If users want a short or stable name, they can choose that name as the profile registry key.

There is no compatibility requirement for aliases because the feature has just landed and has not become a stable user-facing contract.

## Requirements

- Remove profile alias support from registry parsing and selection.
  - Delete `ProfileAlias` and alias maps/resolution paths.
  - Remove `[alias]` from the `profiles.toml` schema.
  - Do not support alias-to-alias or alias-to-profile indirection.
- Keep profile registry semantics simple.
  - Supported schema:
    ```toml
    default = "coder"

    [profile]
    coder = "profiles/coder.nix"
    researcher = "profiles/researcher.nix"
    ```
  - Table form may remain:
    ```toml
    [profile.coder]
    path = "profiles/coder.nix"
    description = "Coding profile"
    ```
  - `default` must name a profile entry directly.
  - Unqualified defaults resolve within the declaring source.
  - Source-qualified defaults such as `user:coder` may remain if already implemented and useful.
- Keep existing selector behavior for real profiles.
  - explicit path / `path:<path>`
  - discovered unqualified names
  - `default`
  - source-qualified names such as `project:coder`
  - ambiguity fail-closed
  - TUI manifest-cascade opt-out
- Update docs and tests.
  - Remove alias examples and alias-specific tests.
  - Add/keep coverage proving default points directly at a profile entry.
  - Diagnostics for a missing default target should mention the missing profile, not alias behavior.

## Acceptance criteria

- `profiles.toml` no longer accepts or documents `[alias]` as a supported feature.
- `ProfileRegistry` has no alias state or alias resolution path.
- Existing CLI/TUI profile selection works with direct profile entries and defaults.
- Ambiguous unqualified real profile names still fail closed.
- Docs describe only entries + default.
- Focused manifest/tui/pod/client profile tests pass.
- `cargo fmt --check`
- Relevant checks pass.
