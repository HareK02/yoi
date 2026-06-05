---
id: 20260605-203006-migrate-ticket-storage-to-yoi-tickets
slug: migrate-ticket-storage-to-yoi-tickets
title: Migrate Ticket storage to .yoi/tickets
status: open
kind: task
priority: P1
labels: [ticket, migration, storage]
created_at: 2026-06-05T20:30:06Z
updated_at: 2026-06-05T21:43:54Z
assignee: null
legacy_ticket: null
---

## Background

The active Ticket storage should move from top-level `work-items/` to `.yoi/tickets/` so Yoi project orchestration state lives under `.yoi/` with workflows and Ticket config.

This is a project-record migration. Preserve all existing Ticket ids, thread history, artifacts, and resolutions.

## Requirements

- Move active storage:

```text
work-items/open/    -> .yoi/tickets/open/
work-items/pending/ -> .yoi/tickets/pending/
work-items/closed/  -> .yoi/tickets/closed/
```

- Use `git mv` or equivalent tracked moves so history remains inspectable.
- Update code defaults/tests/docs/workflows to refer to `.yoi/tickets` as active storage.
- Update `.yoi/ticket.config.toml` examples/defaults to `root = ".yoi/tickets"`.
- Update worktree workflow guidance if necessary: child worktrees still exclude `.yoi`; Ticket mutation remains main-workspace/orchestrator authority.
- Ensure `.yoi/tickets` is tracked project state, not ignored generated memory.
- Adjust `.gitignore` if needed so `.yoi/tickets`, `.yoi/workflow`, `.yoi/knowledge`, and `.yoi/ticket.config.toml` can be tracked while generated `.yoi/memory` remains ignored.
- Do not rewrite historical thread/artifact body references unless they are current docs/config paths. Historical mentions of `work-items/` may remain as history.

## Non-goals

- Removing `tickets.sh`; handled later.
- Changing Ticket ids/slugs/status semantics.
- External tracker migration.
- Scheduler/lease/queue automation.

## Acceptance criteria

- All active Ticket records are under `.yoi/tickets/`.
- Top-level `work-items/` no longer exists as active storage.
- `yoi ticket list/show/doctor` works against `.yoi/tickets/`.
- Ticket tools and TUI role actions use `.yoi/tickets/` through configured backend root.
- Documentation no longer tells users to use `work-items/` as active storage.
- `git status` shows intentional tracked moves, not delete/recreate loss of records.
- Validation passes:
  - `yoi ticket doctor`
  - transitional `./tickets.sh doctor` only if still present and intentionally updated;
  - `cargo check --workspace --all-targets`;
  - `cargo fmt --check`;
  - `git diff --check`.

## Dependency

Prefer after:

- `yoi-ticket-cli-parity`
- `builtin-yoi-local-ticket-backend-config`
