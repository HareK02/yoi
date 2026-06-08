---
id: 20260605-203006-builtin-yoi-local-ticket-backend-config
slug: builtin-yoi-local-ticket-backend-config
title: Builtin yoi_local Ticket backend config
status: closed
kind: task
priority: P1
labels: [ticket, backend, config]
created_at: 2026-06-05T20:30:06Z
updated_at: 2026-06-05T21:26:56Z
assignee: null
---

## Background

The Ticket backend should be configured as an explicit built-in Yoi local backend rather than an implicit generic local root.

The desired config is:

```toml
[backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"
```

This makes the backend an explicit Yoi product capability and prepares for removing `tickets.sh` and moving storage under `.yoi/`.

## Requirements

- Extend `.yoi/ticket.config.toml` backend schema from `kind = "local"` toward `provider = "builtin:yoi_local"`.
- Support `provider = "builtin:yoi_local"` as the canonical spelling.
- Decide whether old `kind = "local"` is rejected immediately or accepted only as a short transitional alias. Prefer avoiding long-term compatibility aliases.
- Default backend provider should become `builtin:yoi_local`.
- Default backend root should become `.yoi/tickets` once migration is ready, or support a transition mode if this ticket lands before storage migration.
- Update Ticket tools / Pod Ticket feature adapter to use the configured provider/root.
- Update role launcher/TUI paths if they inspect backend diagnostics.
- Keep backend root path containment and fail-closed behavior.
- Do not auto-create active storage unless the CLI command explicitly creates/migrates records.

## Non-goals

- Moving existing records; handled by `migrate-ticket-storage-to-yoi-tickets`.
- Removing `tickets.sh`; handled by `remove-tickets-sh`.
- External provider implementation.
- GitHub/Linear/Jira/MCP backend support.
- TUI UI changes.

## Acceptance criteria

- `.yoi/ticket.config.toml` with `provider = "builtin:yoi_local"` parses and resolves.
- Missing config defaults to the selected built-in backend semantics.
- Tests cover provider parsing, unsupported provider diagnostics, relative root resolution, missing/unusable root behavior, and Pod Ticket feature adapter integration.
- Docs/examples use `provider = "builtin:yoi_local"`.
- `cargo test -p ticket` and focused Pod Ticket tests pass.
- `cargo check --workspace --all-targets`, `cargo fmt --check`, `git diff --check`, and Ticket doctor validations pass.

## Dependency

Prefer after `yoi-ticket-cli-parity`, so validation and migration can use `yoi ticket doctor`.
