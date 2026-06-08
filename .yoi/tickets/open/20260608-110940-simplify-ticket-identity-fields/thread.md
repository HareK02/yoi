<!-- event: create author: LocalTicketBackend at: 2026-06-08T11:09:40Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-08T11:15:09Z -->

## Decision

## Scope expansion: simplify core Ticket frontmatter, not only identity

Expand this Ticket beyond identity cleanup. The same simplification pass should also review and reduce the core Ticket frontmatter shape.

### Unify `status` and `workflow_state`

The current split between local `status` and `workflow_state` creates an invalid two-axis state model. Combinations such as `status: closed` with `workflow_state: inprogress` are not meaningful.

Direction:

- Use one canonical lifecycle state field.
  - Final name can be `state` or retained as `workflow_state`, but there should not be both `status` and `workflow_state` as independent current fields.
- Treat `open` as a derived range of states rather than a stored independent status.
  - Example: `open = planning | ready | queued | inprogress | done`.
  - Terminal closed state should be represented by the same lifecycle model, e.g. `closed` or equivalent.
- Decide whether `done` and `closed` remain distinct.
  - `done`: implementation/review/merge complete, close/resolution may still be pending.
  - `closed`: resolution recorded and Ticket lifecycle ended.
- Revisit or remove the `pending` bucket/status. If a concept is still needed, define it as a lifecycle state such as `deferred` / `parked`, not as a second axis.
- Update path layout expectations if needed.
  - Directory buckets may be derived caches or UI grouping, but frontmatter should not duplicate contradictory state.

### Remove `kind`

`kind` currently behaves like a single typed category but is a freeform string with unclear management and little semantic authority. Most Tickets are work items and can be described by their title/body.

Direction:

- Remove `kind` from the required/current schema unless a small typed enum with clear semantics is explicitly justified.
- Do not keep required freeform `kind`.
- If the title follows a convention such as `area: verb object`, it already communicates the work class better than an unconstrained `kind: task` field.

### Remove `labels`

`labels` are useful for search, but without a registry/canonical vocabulary they create inconsistent taxonomy and can be confused with state, relation, risk, component, or priority.

Direction:

- Remove `labels` from the core required/current schema in this simplification pass.
- Do not use labels to represent state, dependency, blockers, risk, priority, or ownership.
- If tag-like search is needed later, add it as a separate feature with a project-managed registry/canonicalization model.
- Prefer descriptive titles and body content for search. A title convention such as `component: concise action` can carry much of the useful categorization without an unmanaged labels field.

### Resulting target direction

The target core Ticket identity/frontmatter may be as small as:

```yaml
id: 20260608-103842   # or path-derived primary key
title: component: concise action
state: planning
created_at: ...
updated_at: ...
```

Additional fields should justify a concrete behavior. Search/display hints without management authority should be removed or moved to separately designed features.

---
