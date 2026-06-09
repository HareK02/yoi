---
title: "Remove legacy Ticket schema fields and intake compatibility"
state: "closed"
created_at: "2026-06-08T10:38:42Z"
updated_at: "2026-06-08T12:49:24Z"
queued_by: "workspace-panel"
queued_at: "2026-06-08T11:21:33Z"
---

## Background

The local Ticket records have been migrated away from legacy `workflow_state: intake` and legacy `preflight` vocabulary. Current open Tickets use explicit `workflow_state: planning`, and open Ticket `item.md` records no longer rely on `needs_preflight`.

Two legacy frontmatter fields/compatibility paths remain in the code/schema:

- `legacy_ticket`: currently a required frontmatter field, but it has no behavioral effect beyond being parsed, stored, and displayed. It is usually `null` and does not validate or resolve any reference.
- `needs_preflight`: legacy compatibility metadata from the removed preflight concept. It should no longer be authored or exposed as current Ticket state.

The parser also still accepts `workflow_state: intake` as a compatibility alias for `planning`. After local migration, this compatibility should be removed rather than preserved as a permanent runtime surface.

## Goal

Remove obsolete Ticket schema compatibility: delete `legacy_ticket` and `needs_preflight` from the current Ticket schema/tool/API, and reject `workflow_state: intake` instead of normalizing it to `planning`.

## Requirements

- Remove `legacy_ticket` from current Ticket frontmatter requirements.
  - New Tickets should not write `legacy_ticket`.
  - `ticket doctor` should not require `legacy_ticket`.
  - `TicketCreate` / tool input should no longer expose `legacy_ticket` unless a deliberate migration-only path is kept separately.
  - `TicketShow` / list output should not expose it as a current field.
  - Existing local records should be migrated to remove the field before or as part of this change.
- Remove `needs_preflight` from current Ticket schema and tool/API output.
  - New Tickets should not write it.
  - Parser should not treat it as a current known field.
  - `TicketShow` / `TicketList` should not show `needs_preflight` as current metadata.
  - Any remaining historical mentions in `thread.md` stay audit history only.
- Remove `workflow_state: intake` compatibility.
  - `TicketWorkflowState::parse` or equivalent parser should reject `intake`.
  - Missing `workflow_state` compatibility should be reconsidered; if retained temporarily, it should default to `planning` only as a bounded migration fallback, not reintroduce `intake` vocabulary.
  - Tests should cover that `intake` is invalid.
- Update tests, fixtures, prompts, docs, and Ticket examples that still include:
  - `legacy_ticket` as required frontmatter;
  - `needs_preflight` as a Ticket field;
  - `workflow_state: intake` as accepted/current state.
- Update local Ticket records under `.yoi/tickets` so current `item.md` files pass the stricter schema.
- Keep historical `thread.md` text intact unless a test fixture requires explicit migration; old thread mentions are audit history, not schema input.

## Non-goals

- Adding typed Ticket relation metadata; that is covered by `typed-ticket-relation-metadata`.
- Reintroducing a generic external issue link field. If needed later, it should be designed as a typed relation/external reference feature, not as the current inert `legacy_ticket` stub.
- Rewriting all closed historical thread prose.

## Acceptance criteria

- New Ticket frontmatter omits `legacy_ticket` and `needs_preflight`.
- `ticket doctor` passes without requiring `legacy_ticket`.
- `TicketCreate`, `TicketShow`, and `TicketList` no longer expose `legacy_ticket` or `needs_preflight` as current fields.
- `workflow_state: intake` is rejected by parser/doctor/tests.
- Existing current Ticket records are migrated to the stricter schema.
- Tests covering legacy `needs_preflight` / `legacy_ticket` required frontmatter are removed or rewritten.
- Focused tests, `target/debug/yoi ticket doctor`, `cargo fmt --check`, and `git diff --check` pass.
