---
id: 20260605-203006-yoi-local-ticket-backend-migration
slug: yoi-local-ticket-backend-migration
title: Yoi-local Ticket backend migration
status: closed
kind: task
priority: P1
labels: [ticket, backend, migration, cli]
created_at: 2026-06-05T20:30:06Z
updated_at: 2026-06-05T22:13:53Z
assignee: null
legacy_ticket: null
---

## Background

Ticket development is now user-facing through TUI role actions, typed Ticket tools, Ticket workflows, and the Rust `ticket` crate. The old `tickets.sh` CLI and top-level `work-items/` storage are no longer the right long-term authority boundary.

The desired end state is:

- `yoi` binary owns Ticket operations.
- Ticket backend is configured as a built-in Yoi local backend.
- Ticket records live under `.yoi/tickets/`.
- `tickets.sh` is removed.
- `work-items/` is removed after migration.

This is an umbrella for the migration. Child tickets should land in order so the repository remains operable at each step.

## Target model

```toml
# .yoi/ticket.config.toml

[backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"
```

Storage:

```text
.yoi/tickets/{open,pending,closed}/<id>/
  item.md
  thread.md
  artifacts/
  resolution.md   # closed Tickets only
```

User-facing operations:

```text
yoi ticket create
yoi ticket list
yoi ticket show
yoi ticket comment
yoi ticket review
yoi ticket status
yoi ticket close
yoi ticket doctor
```

## Child tickets

1. `yoi-ticket-cli-parity`
   - Add `yoi ticket ...` CLI parity over the Rust Ticket backend.
   - Keep existing storage initially.

2. `builtin-yoi-local-ticket-backend-config`
   - Add `provider = "builtin:yoi_local"` backend config and default root `.yoi/tickets`.
   - Preserve compatibility with existing storage during transition.

3. `migrate-ticket-storage-to-yoi-tickets`
   - Move existing records from `work-items/` to `.yoi/tickets/`.
   - Update docs/workflows/tests/defaults.

4. `remove-tickets-sh`
   - Remove `tickets.sh` after `yoi ticket ...` and `.yoi/tickets` are authoritative.

## Requirements

- Do not leave two authoritative mutation paths.
- Do not shell out from product code to `tickets.sh`.
- Preserve existing Ticket history and artifacts.
- Preserve `git history + Ticket files` as the durable project record.
- Keep child worktrees excluding `.yoi`; orchestration state remains in the main workspace.
- Keep `.yoi/tickets`, `.yoi/workflow`, and `.yoi/ticket.config.toml` tracked project state.
- Do not mix this migration with scheduler/lease/TUI dashboard work.

## Acceptance criteria

- All child tickets are closed.
- `yoi ticket ...` is the documented direct CLI path.
- Ticket tools and TUI role actions use the configured built-in backend.
- Existing records are under `.yoi/tickets/`.
- `work-items/` no longer exists as active storage.
- `tickets.sh` no longer exists.
- Repository validation uses `yoi ticket doctor` instead of `./tickets.sh doctor`.
- Docs explain user-facing TUI/Ticket tool workflows first and local backend details only as implementation details.

## Non-goals

- External tracker integration.
- GitHub/Linear/Jira/MCP backend support.
- Scheduler/lease/queue automation.
- Stateful workflow engine.
- Changing Ticket content semantics beyond storage/config migration.
