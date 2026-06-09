---
title: "Reorganize documentation information architecture"
state: "closed"
created_at: "2026-06-01T11:00:26Z"
updated_at: "2026-06-01T13:22:50Z"
---

## Background

The repository documentation still mixes current design intent, historical plans, external research, and implementation notes. The next documentation pass should make the maintained docs answer why Yoi is shaped this way, while moving or deleting research/log material that is not useful as stable developer documentation.

This work item covers the top-level documentation information architecture only. Crate-level README cleanup is tracked separately by `crate-readme-boundaries`.

## Requirements

- Keep repository documentation focused on current Yoi design intent and development workflow.
- Prefer durable Why over implementation details that code, tickets, or git history already own.
- Do not preserve stale plan/research material merely because it exists.
- Treat `docs/.local/` as the home for private or non-authoritative notes that should not be part of the public documentation surface.

## Acceptance criteria

- Root `README.md` points developers to the maintained documentation map.
- Maintained design docs live under `docs/design/` and cover the major Yoi design boundaries.
- Maintained development workflow docs live under `docs/development/`.
- `docs/ref/` and `docs/research/` are no longer part of the repository documentation surface.
- `docs/plan/` is removed or reduced so old plans are not mistaken for current design authority.
- Validation includes `./tickets.sh doctor` and `git diff --check`.
