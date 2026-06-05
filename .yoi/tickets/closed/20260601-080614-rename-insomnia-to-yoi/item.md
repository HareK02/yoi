---
id: 20260601-080614-rename-insomnia-to-yoi
slug: rename-insomnia-to-yoi
title: Rename project from Insomnia to Yoi
status: closed
kind: task
priority: P1
labels: [branding, rename, release-hygiene]
created_at: 2026-06-01T08:06:14Z
updated_at: 2026-06-01T09:49:11Z
assignee: null
legacy_ticket: null
---

## Background

The project name `Insomnia` collides with Kong's established Insomnia API/developer tooling product, including public product naming, package/binary naming, CLI/tooling, plugins, documentation, and `.insomnia` project data. The project will adopt **Yoi** as the new public identity, from the archaic Japanese word **夜居**: staying present at night / night duty / remaining at the workplace until late.

Branding rationale and adoption checks are recorded in `docs/branding.md`. Nixpkgs name availability is the required distribution check and currently passes; broader package-manager checks found only an npm package-name collision, which is not a current distribution blocker.

## Requirements

- Rename the public product identity from Insomnia to Yoi.
- Rename user-facing CLI/package/config/runtime surfaces that currently expose `insomnia` where doing so is not purely internal implementation detail.
- Prefer a clean public identity; do not add `insomnia` compatibility aliases, legacy env aliases, prompt/profile aliases, or old-path handling.
- Preserve existing project behavior while renaming, without speculative redesign.
- Keep generated/personal state handling safe; do not expose secrets or private session content during broad rename audits.
- Update documentation, prompts, diagnostics, package metadata, and release-facing text consistently.

## Acceptance criteria

- The installable main binary/package identity is `yoi` rather than `insomnia`.
- Nix package/app outputs expose `yoi` and no longer center the long-term public output on `insomnia`.
- User data/config/workspace directories and environment variable prefixes are renamed directly, without old-name compatibility aliases.
- User-visible docs, prompts, diagnostics, and command help refer to Yoi where appropriate.
- Repository references to `insomnia` are either renamed, intentionally retained as historical/internal/generated context, or listed in the implementation report as deferred/intentional.
- Tests and packaging checks relevant to renamed surfaces pass.
