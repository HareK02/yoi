---
title: "Yoi ticket CLI parity"
state: "closed"
created_at: "2026-06-05T20:30:06Z"
updated_at: "2026-06-05T20:58:21Z"
---

## Background

Before `tickets.sh` can be removed, the `yoi` binary must provide direct Ticket CLI operations over the Rust Ticket backend.

This ticket moves the direct local CLI surface from shell script compatibility into the product binary. It should use `crates/ticket` APIs directly, not shell out to `tickets.sh`.

## Requirements

Add `yoi ticket ...` subcommands with parity for the operations currently used through the local compatibility CLI:

- `yoi ticket create --title "..." [--slug slug] [--kind task] [--priority P2] [--label a,b]`
- `yoi ticket list [--status open|pending|closed|all]`
- `yoi ticket show <id-or-slug>`
- `yoi ticket comment <id-or-slug> [--role comment|plan|decision|implementation_report] [--file path|--message text]`
- `yoi ticket review <id-or-slug> --approve|--request-changes [--file path|--message text]`
- `yoi ticket status <id-or-slug> open|pending|closed`
- `yoi ticket close <id-or-slug> [--resolution text|--file path]`
- `yoi ticket doctor`

Use `ticket::TicketBackend` / `LocalTicketBackend` or the configured backend resolver if available.

## Scope

- Product CLI belongs in the `insomnia`/top-level `yoi` binary crate, consistent with current CLI ownership.
- CLI output should be stable enough for human use and tests, but does not need to preserve exact `tickets.sh` output formatting.
- Preserve existing Ticket file compatibility.
- Keep `tickets.sh` in place for this ticket; removal happens later.
- Keep active storage where it is for now unless the backend config ticket has already landed.

## Non-goals

- Moving storage to `.yoi/tickets/`.
- Removing `tickets.sh`.
- External tracker backends.
- TUI changes.
- Scheduler/lease/queue.

## Acceptance criteria

- `yoi ticket --help` documents available subcommands.
- Each required operation works through Rust backend APIs without invoking `tickets.sh`.
- `yoi ticket doctor` can replace `./tickets.sh doctor` in validation after the migration.
- Tests cover create/list/show/comment/review/status/close/doctor in temp roots or fixtures.
- Files written by `yoi ticket ...` are readable by existing backend/tests.
- `cargo test` for affected crates passes.
- `cargo check --workspace --all-targets`, `cargo fmt --check`, `git diff --check`, and both `yoi ticket doctor` and `./tickets.sh doctor` pass during the transition.

## Follow-up

- `builtin-yoi-local-ticket-backend-config`
- `migrate-ticket-storage-to-yoi-tickets`
- `remove-tickets-sh`
