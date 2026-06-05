---
id: 20260531-064550-rename-pod-command-crate-to-insomnia
slug: rename-pod-command-crate-to-insomnia
title: CLI: rename pod-command crate to insomnia
status: closed
kind: task
priority: P2
labels: [cli, pod, cargo]
created_at: 2026-05-31T06:45:50Z
updated_at: 2026-05-31T06:49:44Z
assignee: null
legacy_ticket: null
---

## Background

The single-binary CLI migration introduced a small helper crate named `pod-command` for typed Pod runtime command resolution (`current_exe() + ["pod"]`, plus executable-only `INSOMNIA_POD_COMMAND` override).

The user decided this helper crate should be named after the installed/runtime binary surface, `insomnia`, rather than `pod-command`.

## Requirements

- Rename the `pod-command` crate/package to `insomnia`.
- Keep the existing typed runtime command behavior intact:
  - default runtime command is current executable plus `pod` prefix arg;
  - `INSOMNIA_POD_COMMAND` remains executable-only and is not shell parsed.
- Update workspace membership, workspace dependencies, crate dependencies, imports, lockfile, and tests.
- Do not rename the `tui` package/crate in this ticket.
- Do not reintroduce an `insomnia-pod` binary/alias.
- Do not change Pod runtime process model, flags, or protocol.

## Acceptance criteria

- No active code or Cargo metadata references the `pod-command` crate/package.
- The helper crate is available as package/crate `insomnia`.
- Existing spawn/restore code uses the renamed crate without behavior changes.
- Focused tests for the renamed helper crate pass.
- `cargo fmt --check`, relevant `cargo test`/`cargo check`, `./tickets.sh doctor`, and `git diff --check` pass.
