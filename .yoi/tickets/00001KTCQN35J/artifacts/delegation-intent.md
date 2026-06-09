# Delegation intent: remove `tickets.sh`

## Classification

`implementation-ready` after storage migration.

The prerequisite migration is complete: active Ticket records live under `.yoi/tickets/`, the Rust `yoi ticket` CLI has parity, and `tickets.sh` is only a transitional shim. Keeping it now leaves an unnecessary second mutation path.

## Intent

Delete `tickets.sh` and update active documentation, project instructions, tests, and validation references so the repository uses the typed Rust Ticket backend through `yoi ticket ...` and Ticket tools.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/remove-tickets-sh`
- branch: `work/remove-tickets-sh`

This ticket will edit tracked `.yoi/tickets` records and may edit `.yoi/ticket.config.toml` only if necessary. Do not read or edit `.yoi/memory/`; it is ignored generated memory state and is not part of this cleanup.

## Requirements

- Delete repository-root `tickets.sh`.
- Remove or update every active doc/workflow/test/project-instruction reference that tells users, maintainers, or agents to run `./tickets.sh`.
- Replace validation examples with `yoi ticket doctor`.
- Replace lifecycle examples with `yoi ticket create/list/show/comment/review/status/close/doctor`.
- Remove shell compatibility tests that execute `tickets.sh`, or port them to Rust backend / `yoi ticket ...` coverage.
- Ensure no production code shells out to `tickets.sh`.
- Preserve historical Ticket thread/artifact references when they are closed historical context; do not mass-rewrite old records just for path hygiene.
- Keep `.yoi/tickets/` as the active storage root and avoid reintroducing `work-items/` as mutable storage.
- Update `work-items/README.md` so it no longer points to the transitional script.

## Active references to inspect/update

Current active references include at least:

- `AGENTS.md`
  - currently says `.yoi/tickets/` and `tickets.sh` are authoritative and lists `./tickets.sh ...` commands.
  - Update to `yoi ticket ...` as the command path; `.yoi/tickets/` remains storage/project-record authority.
- `README.md`
  - update dogfooding/doctor examples from `./tickets.sh doctor` to `yoi ticket doctor`.
- `docs/development/work-items.md`
  - remove the `tickets.sh` maintainer CLI section or turn it into a historical note only if needed.
  - normal user path remains TUI role actions / Ticket tools / workflows.
- `docs/development/validation.md`
  - update validation command examples.
- `crates/workflow/README.md`
- `crates/lint-common/README.md`
- tests under `crates/ticket` that mention or execute `tickets.sh`.

Historical/closed records and old report artifacts may still mention `tickets.sh` or `work-items/`; leave them alone unless an active test/doc depends on them.

## Non-goals

- Moving storage; already done.
- Adding Ticket backend features.
- TUI UI changes.
- External tracker/provider support.
- Rewriting closed historical Ticket threads/artifacts.

## Validation

Run at least:

- `cargo test -p ticket`
- `cargo test -p yoi ticket`
- `cargo test -p pod ticket --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor` or built binary equivalent

Run `nix build .#yoi --no-link` if feasible.

Also manually check:

- `test ! -e tickets.sh`
- `rg "tickets\.sh|./tickets.sh"` only returns closed historical records/report artifacts, or no active references.
- No active docs/workflows/AGENTS instructions present `tickets.sh` as a command.
- No tests or production code execute `./tickets.sh`.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- deleted script path;
- tests/docs/instructions updated;
- remaining `tickets.sh` references and why they are historical/acceptable, if any;
- validation results;
- whether the umbrella `yoi-local-ticket-backend-migration` can be closed.
