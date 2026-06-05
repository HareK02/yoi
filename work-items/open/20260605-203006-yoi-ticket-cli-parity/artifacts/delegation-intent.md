# Delegation intent: yoi ticket CLI parity

## Intent

Add `yoi ticket ...` CLI operations over the Rust Ticket backend so direct Ticket mutation is owned by the `yoi` binary rather than `tickets.sh`.

This is the first step in the Yoi-local Ticket backend migration. `tickets.sh` remains in place for this ticket only as transitional compatibility and validation reference.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/yoi-ticket-cli-parity`
- branch: `work/yoi-ticket-cli-parity`

## Requirements

Implement these subcommands in the top-level `yoi` binary crate:

- `yoi ticket create --title "..." [--slug slug] [--kind task] [--priority P2] [--label a,b]`
- `yoi ticket list [--status open|pending|closed|all]`
- `yoi ticket show <id-or-slug>`
- `yoi ticket comment <id-or-slug> [--role comment|plan|decision|implementation_report] [--file path|--message text]`
- `yoi ticket review <id-or-slug> --approve|--request-changes [--file path|--message text]`
- `yoi ticket status <id-or-slug> open|pending|closed`
- `yoi ticket close <id-or-slug> [--resolution text|--file path]`
- `yoi ticket doctor`

Use `crates/ticket` backend APIs directly. Do not shell out to `tickets.sh`.

## Current code map

- `crates/yoi/src/main.rs`
  - Owns top-level CLI parsing and dispatch.
  - Existing subcommands include `pod`, `keys`, and `memory lint`.
  - Add `ticket` parsing/dispatch here or in a new module such as `crates/yoi/src/ticket_cli.rs`.
- `crates/yoi/Cargo.toml`
  - Add dependency on `ticket` if needed.
- `crates/ticket/src/lib.rs`
  - Ticket domain/backend APIs: `LocalTicketBackend`, `TicketBackend`, `NewTicket`, `NewTicketEvent`, `TicketReview`, status/review/event types, doctor reports.
- `crates/ticket/src/config.rs`
  - Workspace ticket config and backend root defaults. Use if it helps locate backend root; otherwise current active storage is still `work-items/` for this ticket.
- `tickets.sh`
  - Compatibility/reference only. Do not invoke it from Rust code.

## Scope notes

- Active storage stays where it is for this ticket (`work-items/` unless config already directs otherwise).
- Keep `tickets.sh` in place.
- CLI output does not need exact `tickets.sh` formatting, but should be understandable and stable enough for tests.
- Prefer concise plain text output for human CLI; JSON is not required unless already easy.
- `yoi ticket doctor` should be usable as the future validation command.

## Non-goals

- Moving storage to `.yoi/tickets/`.
- Removing `tickets.sh`.
- Adding external tracker backends.
- TUI changes.
- Scheduler/lease/queue.
- Broad CLI framework refactor.

## Edge cases / validation expectations

- `comment` / `review` should accept exactly one body source: `--file` or `--message`, or provide a clear error.
- `review` requires exactly one of `--approve` / `--request-changes`.
- `close` requires resolution text/file or a clear error if the backend cannot close without a resolution.
- `status closed` should preserve backend semantics. If direct status-to-closed cannot write a resolution, either reject it with guidance to use `close` or keep parity only if existing backend supports it safely.
- `doctor` should print diagnostics and return non-zero on failures.
- `--help` for `yoi ticket` should document subcommands.

## Validation

Run at least:

- focused `cargo test -p yoi ticket` tests;
- `cargo test -p ticket`;
- `cargo check --workspace --all-targets`;
- `cargo fmt --check`;
- `git diff --check`;
- `./tickets.sh doctor`;
- built binary `yoi ticket doctor` against the repository if possible.

Run `nix build .#yoi --no-link` if feasible.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- implemented command surface;
- backend root resolution behavior;
- tests/validation results;
- known compatibility gaps from `tickets.sh`, if any;
- whether ready for external review.
