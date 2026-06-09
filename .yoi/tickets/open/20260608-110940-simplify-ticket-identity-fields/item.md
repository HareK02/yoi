---
id: '20260608-110940-simplify-ticket-identity-fields'
slug: 'simplify-ticket-identity-fields'
title: 'Simplify Ticket identity to timestamp key and title'
status: 'open'
kind: 'task'
priority: 'P1'
labels: ['ticket', 'schema', 'identity', 'migration', 'orchestrator']
workflow_state: 'inprogress'
created_at: '2026-06-08T11:09:40Z'
updated_at: '2026-06-09T02:15:46Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-09T02:13:30Z'
---

## Background

The current Ticket identity/frontmatter shape carries overlapping identity/display fields:

```yaml
id: 20260608-103842-remove-legacy-ticket-schema-fields
slug: remove-legacy-ticket-schema-fields
title: Remove legacy Ticket schema fields and intake compatibility
```

This makes the ID and slug semantically rich enough that agents may infer meaning from the identifier instead of reading the authoritative Ticket body/thread. It also duplicates the role of `title`: the slug is a second human-readable label, and the current ID is timestamp plus slug.

A GitHub-Issue-like model may be cleaner:

```text
primary key: opaque timestamp/counter ID
title: human-readable summary
body/thread: authoritative requirements and history
```

The desired direction is to make Ticket identity mostly opaque and force both humans and agents to follow `id -> TicketShow -> body/thread/artifacts` for real meaning.

## Goal

Redesign Ticket identity/frontmatter around a timestamp primary key plus title, removing slug as a required/current identity field and avoiding title-derived meaning in the canonical ID.

## Proposed direction

- Use a timestamp-based primary key as the canonical Ticket ID.
  - Example: `20260608-103842`.
  - If collision is possible, use a deterministic suffix such as `20260608-103842-2` or a higher-resolution timestamp.
- Store Tickets under directories named by that primary key only:
  - `.yoi/tickets/open/20260608-103842/`
- Keep `title` as the human-readable display summary.
- Remove `slug` from required frontmatter and from canonical lookup semantics.
- Consider whether `id` must remain in frontmatter or can be derived from the directory name.
  - Option A: keep `id: 20260608-103842` for file self-description and doctor path checks.
  - Option B: remove `id` from frontmatter and make directory name authoritative.
- Do not include title/slug words in the canonical ID.

## Requirements

- Audit current uses of `id`, `slug`, and title-derived directory names across:
  - Ticket backend create/list/show/query;
  - Ticket tools;
  - CLI;
  - Panel/TUI;
  - role/session registry and local claims;
  - Orchestrator prompts/workflows;
  - future relation metadata assumptions.
- Define the canonical Ticket reference format.
  - Prefer opaque timestamp/counter ID.
  - Exact lookup should use canonical ID.
  - If title search is needed, provide search/list UX rather than making slug canonical.
- Remove `slug` as a required current field.
  - New Tickets should not require or write `slug` unless a deliberate display/search alias is kept separately.
  - `ticket doctor` should not require `slug` if the new schema removes it.
  - Existing local records should be migrated.
- Decide whether `id` remains in frontmatter.
  - If retained, it must be the opaque primary key and match the directory name.
  - If removed, update doctor/show/list to derive it from the directory path.
- Update `TicketShow` / `TicketList` / Panel display to present `id + title` rather than `id + slug + title`.
- Update query behavior.
  - Exact ID lookup remains.
  - Slug lookup should be removed or treated as legacy/migration-only.
  - Title search, if needed, should be explicit and bounded rather than ambiguous exact slug matching.
- Update typed Ticket relations design to reference canonical ID, not slug.
- Update AI/Orchestrator guidance:
  - do not infer Ticket meaning from ID/title alone;
  - always read `TicketShow` body/thread/artifacts before routing, accepting, returning to planning, or implementing.
- Migrate current local `.yoi/tickets` records and path layout.
  - Example: `20260608-103842-remove-legacy-ticket-schema-fields` -> `20260608-103842`.
  - Preserve Git history through normal commit; no special backward compatibility for unreleased local data unless explicitly required.

## Non-goals

- Designing full typed Ticket relation metadata; this ticket should only ensure relation metadata will use canonical opaque IDs later.
- Removing `title`.
- Replacing full-text/list/search UX with ID-only workflows.
- Preserving slug as a permanent compatibility alias unless explicitly justified.

## Acceptance criteria

- New Ticket records use an opaque timestamp/counter primary key and title, without title words in the canonical ID.
- `slug` is no longer a required current frontmatter field or canonical lookup key.
- `TicketList` and `TicketShow` display the simplified identity clearly.
- Agents cannot reasonably infer Ticket requirements from canonical ID alone.
- Ticket lookup, Panel actions, role/session claims, and future relation metadata use canonical ID.
- Local Ticket records are migrated to the simplified identity schema.
- Tests cover create, lookup, doctor, duplicate/collision handling, and migration-relevant parsing.
- `target/debug/yoi ticket doctor`, focused tests, `cargo fmt --check`, and `git diff --check` pass.
