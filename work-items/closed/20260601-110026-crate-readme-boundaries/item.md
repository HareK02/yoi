---
id: 20260601-110026-crate-readme-boundaries
slug: crate-readme-boundaries
title: Standardize crate README responsibility boundaries
status: closed
kind: task
priority: P2
labels: [docs]
created_at: 2026-06-01T11:00:26Z
updated_at: 2026-06-01T13:22:51Z
assignee: null
legacy_ticket: null
---

## Background

The crate README files are uneven and several important crates have no README at all. They should be standardized as thin responsibility-boundary documents rather than API references.

This work item follows `docs-information-architecture` and should link crate README files to the maintained design/development docs where appropriate.

## Requirements

- Each crate README should explain the crate's role, boundaries, key entry points, and relevant design docs.
- README files should avoid public type inventories, method lists, and implementation details that are likely to drift.
- Important crates without README files should get concise responsibility-boundary README files.
- Existing stale names or stale architectural claims should be removed.

## Acceptance criteria

- Existing crate README files follow a consistent role/boundary-oriented shape.
- Important crates without README files have a short README.
- Crate README files link to `docs/design/` or `docs/development/` instead of duplicating long design content.
- Validation includes `./tickets.sh doctor` and `git diff --check`.
