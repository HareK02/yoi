---
id: 20260605-203006-remove-tickets-sh
slug: remove-tickets-sh
title: Remove tickets.sh compatibility CLI
status: closed
kind: task
priority: P1
labels: [ticket, cleanup, cli]
created_at: 2026-06-05T20:30:06Z
updated_at: 2026-06-05T22:13:36Z
assignee: null
legacy_ticket: null
---

## Background

After `yoi ticket ...` has CLI parity and active Ticket storage has moved to `.yoi/tickets/`, the old shell compatibility CLI should be removed.

Keeping `tickets.sh` would leave a second mutation path and force duplicate semantics for create/comment/review/status/close/doctor.

## Requirements

- Delete `tickets.sh`.
- Remove or update every active doc/workflow/test reference that tells users/agents to run `./tickets.sh`.
- Replace validation references with `yoi ticket doctor`.
- Remove shell-specific tests if any, or port them to `yoi ticket ...` / Rust backend tests.
- Ensure no production code shells out to `tickets.sh`.
- Ensure all Ticket mutation paths use `crates/ticket` backend APIs or the `yoi ticket` CLI.
- Preserve historical Ticket thread/artifact mentions if they are closed historical context; do not rewrite old records unnecessarily.

## Non-goals

- Moving storage; must already be complete.
- Adding new Ticket features.
- External tracker support.
- TUI changes beyond documentation/help if needed.

## Acceptance criteria

- `tickets.sh` no longer exists.
- Repository docs/workflows no longer present `tickets.sh` as an active command.
- Validation docs use `yoi ticket doctor`.
- `rg "tickets.sh"` returns only closed historical records or no active references.
- `yoi ticket doctor` passes.
- `cargo check --workspace --all-targets`, `cargo fmt --check`, `git diff --check`, and relevant tests pass.

## Dependencies

Requires:

- `yoi-ticket-cli-parity`
- `builtin-yoi-local-ticket-backend-config`
- `migrate-ticket-storage-to-yoi-tickets`
