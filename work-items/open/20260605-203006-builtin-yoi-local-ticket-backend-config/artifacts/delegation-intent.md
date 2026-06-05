# Delegation intent: builtin yoi_local Ticket backend config

## Classification

`implementation-ready` with a transitional storage-root constraint.

`yoi-ticket-cli-parity` is complete, so the binary now owns direct Ticket CLI operations. The next step is to make the Ticket backend provider explicit as Yoi's built-in local backend without moving storage yet.

## Intent

Extend `.yoi/ticket.config.toml` backend config from implicit/generic local storage toward a canonical built-in provider:

```toml
[backend]
provider = "builtin:yoi_local"
root = "work-items" # transitional until migrate-ticket-storage-to-yoi-tickets
```

This ticket should introduce the provider field and canonical spelling. It must not migrate records to `.yoi/tickets`; that is the next ticket.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/builtin-yoi-local-ticket-backend-config`
- branch: `work/builtin-yoi-local-ticket-backend-config`

## Requirements

- Update Ticket config parsing in `crates/ticket/src/config.rs`.
- Add a backend provider concept with canonical value `builtin:yoi_local`.
- Keep the backend implementation local and bounded to the configured root.
- Preserve current active storage for this ticket:
  - missing config should continue to resolve to `<workspace>/work-items` until the storage migration ticket lands;
  - docs/tests should call this transitional explicitly.
- Support `.yoi/ticket.config.toml` with:

```toml
[backend]
provider = "builtin:yoi_local"
root = "work-items"
```

- Decide and implement legacy handling for existing `kind = "local"`:
  - Accept as a short transitional alias if needed for compatibility/tests, but mark it non-canonical in docs/comments; or
  - Reject with a clear diagnostic if that is cleaner and does not break current tests.
- Prefer new public names such as `TicketBackendProvider::BuiltinYoiLocal` over overloading `TicketBackendKind::Local`.
- Keep unsupported providers fail-closed with bounded diagnostics.
- Update Pod Ticket feature adapter only as needed to use provider/root validation.
- Update examples/docs that currently show `kind = "local"` to use `provider = "builtin:yoi_local"` where active docs are touched.

## Non-goals

- Moving storage to `.yoi/tickets/`.
- Removing `tickets.sh`.
- External provider implementations.
- GitHub/Linear/Jira/MCP backend support.
- TUI UI changes.
- Scheduler/lease/queue automation.
- Changing role profile/launch prompt/workflow semantics.

## Current code map

- `crates/ticket/src/config.rs`
  - Current backend config is `TicketBackendKind::Local` and root defaults to `<workspace>/work-items`.
  - Add provider support here.
- `crates/pod/src/feature/builtin/ticket.rs`
  - Uses `TicketConfig::load_workspace(...)` and `config.backend.root`.
  - Should preserve fail-closed/no-register behavior for malformed config/unusable roots.
- `crates/yoi/src/ticket_cli.rs`
  - Uses Ticket config/backend root for `yoi ticket ...`; make sure provider changes do not break CLI.
- `docs/development/work-items.md`
  - Shows `.yoi/ticket.config.toml` examples; update from `kind = "local"` to `provider = "builtin:yoi_local"` if not already migrated by another change.

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
- `./tickets.sh doctor` during transition

Run `nix build .#yoi --no-link` if feasible.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- final backend config schema;
- legacy `kind = "local"` handling;
- default root behavior and why storage was not moved;
- docs/tests updated;
- validation results;
- whether `migrate-ticket-storage-to-yoi-tickets` can proceed.
