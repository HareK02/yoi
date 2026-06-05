# Delegation intent: migrate Ticket storage to `.yoi/tickets`

## Classification

`implementation-ready` with repository-record migration risk.

The prerequisite `builtin-yoi-local-ticket-backend-config` is complete: Ticket backend config now has canonical provider `builtin:yoi_local` while preserving transitional `work-items` storage. This ticket performs the storage move.

## Intent

Move the active built-in Yoi local Ticket backend storage from repository-root `work-items/` to `.yoi/tickets/` and make that the default/configured root for the Rust Ticket backend.

Target active layout:

```text
.yoi/tickets/{open,pending,closed}/<id>/item.md
.yoi/tickets/{open,pending,closed}/<id>/thread.md
.yoi/tickets/{open,pending,closed}/<id>/artifacts/
```

Target config example:

```toml
[backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"
```

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/migrate-ticket-storage-to-yoi-tickets`
- branch: `work/migrate-ticket-storage-to-yoi-tickets`

This ticket is an explicit exception to the normal child-worktree `.yoi` exclusion pattern because the implementation must add/modify tracked files under `.yoi/`. Do not read or edit `.yoi/memory/`; it is ignored generated memory state and is not part of this migration.

## Requirements

- Move tracked Ticket records from `work-items/` to `.yoi/tickets/` using git-tracked file moves.
- Update `crates/ticket/src/config.rs` defaults so missing Ticket config resolves to `<workspace>/.yoi/tickets`.
- Preserve canonical provider behavior from the previous ticket:
  - `provider = "builtin:yoi_local"` remains the canonical provider;
  - unsupported providers still fail closed;
  - transitional `kind = "local"` handling should not become the documented path.
- Add or update the project `.yoi/ticket.config.toml` if needed so this repository explicitly configures:

```toml
[backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"
```

- Update code/docs/tests that refer to `work-items/` as the active backend root.
- Update `tickets.sh` only as a temporary compatibility/maintainer shim if it remains present after this ticket:
  - it should operate on `.yoi/tickets` by default after the migration;
  - its help text must stop claiming `work-items/` is canonical;
  - keep `WORK_ITEMS_DIR` override if useful for recovery/back-compat.
- Keep Ticket tools, `yoi ticket ...`, TUI role launcher, and Pod Ticket feature registration working against the new root.
- Ensure `target/debug/yoi ticket doctor` / built binary equivalent sees the migrated `.yoi/tickets` records.
- Ensure `./tickets.sh doctor` works during the transition if `tickets.sh` still exists in this ticket.

## Non-goals

- Removing `tickets.sh`; that is the next ticket.
- Rewriting historical thread text/artifacts merely because they mention `work-items/`.
- Migrating generated memory, workflows, Pod sessions, or non-Ticket state.
- Changing Ticket role profile mappings, workflows, role launcher semantics, or TUI UI.
- External Ticket provider support.

## Current code map

- `crates/ticket/src/config.rs`
  - Default root currently remains `work-items`; change to `.yoi/tickets`.
  - Tests should be updated for the new default and explicit config root.
- `crates/pod/src/feature/builtin/ticket.rs`
  - Uses `TicketConfig::load_workspace(...)` and `config.backend.root`; verify diagnostics/root handling remain correct.
- `crates/yoi/src/ticket_cli.rs`
  - Uses Ticket config/backend root for `yoi ticket ...`; verify CLI commands against migrated storage.
- `tickets.sh`
  - Still hardcodes `WORK_ITEMS_DIR=${WORK_ITEMS_DIR:-work-items}` and help text; update for transition or document if intentionally unsupported.
- `docs/development/work-items.md`
  - Update active user documentation to `.yoi/tickets` and `yoi ticket`; `tickets.sh` should remain maintainer/transition-only until removal.
- `AGENTS.md` / project instructions references may still say `work-items/` is authoritative. Update active instructions if in scope so future agents do not mutate the wrong path.

## Migration cautions

- Do not create two active mutable roots. After migration, `.yoi/tickets` is active; `work-items/` should not remain as a second live backend.
- Do not leave open tickets split between old and new roots.
- Do not mass-rewrite old ticket thread prose solely for path hygiene.
- Verify lock-file behavior does not leave `.ticket-backend.lock` in the old root.
- Since the migration moves the ticket records themselves, expect the current ticket directory to move from `work-items/open/...` to `.yoi/tickets/open/...` in the implementation commit.

## Validation

Run at least:

- `cargo test -p ticket config`
- `cargo test -p ticket`
- `cargo test -p pod ticket --lib`
- `cargo test -p yoi ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `target/debug/yoi ticket doctor` or built binary equivalent
- `./tickets.sh doctor` during transition if still present

Run `nix build .#yoi --no-link` if feasible.

Also manually check:

- no active Ticket records remain under `work-items/` unless intentionally retained as a compatibility notice/stub;
- `.yoi/tickets/open`, `.yoi/tickets/pending`, and `.yoi/tickets/closed` contain the migrated records;
- new `yoi ticket create` creates under `.yoi/tickets` in a scratch temp workspace or controlled test fixture, not `work-items`.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- final storage root and config behavior;
- whether `.yoi/ticket.config.toml` was added/updated;
- what happened to `work-items/`;
- whether `tickets.sh` was updated and how;
- docs/tests updated;
- validation results;
- whether `remove-tickets-sh` can proceed.
