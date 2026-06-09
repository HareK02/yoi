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

<!-- event: plan author: intake at: 2026-06-09T00:16:54Z -->

## Plan

## Intake assessment: requirements sync needed

この Ticket は既存 identity simplification に加えて、`status` / `workflow_state` / `kind` / `labels` を含む core Ticket frontmatter simplification へ scope が拡張されている。方向性は明確だが、実装前に schema/API/UX と移行方針として固定すべき判断が残っているため、現時点では `workflow_state: planning` のまま requirements sync が必要。

### 既に決まっていること

- canonical Ticket identity は title-derived slug words を含めない timestamp/counter primary key へ寄せる。
- `slug` は required/current frontmatter および canonical lookup key から外す。
- agents / Orchestrator は ID/title だけから意味を推測せず、routing/実装前に `TicketShow` の body/thread/artifacts を読む。
- core frontmatter simplification は identity だけでなく lifecycle/kind/labels も対象に含める。
- `kind` は required freeform field として残さない方向。
- `labels` は unmanaged taxonomy として core required/current schema から外す方向。

### 実装前に固定すべき open questions

1. lifecycle field の最終名を `state` に変更するか、既存名 `workflow_state` を保持して single lifecycle field にするか。
2. `done` と `closed` を distinct lifecycle states として残すか、close/resolution の表現をどう統合するか。
3. `pending` bucket/status を完全に廃止するか、`deferred` / `parked` などの lifecycle state として置き換えるか。
4. canonical ID を frontmatter に `id` として保持するか、directory name から derive するか。
5. legacy slug/path lookup を migration-only としてどこまで読むか。unreleased local data として一括 migration し、current schema では permanent alias を作らない方針でよいか。
6. `kind` / `labels` removal をこの Ticket の実装 scope に含めて同時 migration するか、identity/lifecycle の破壊的変更と分けるか。

### Risk flags / reviewer focus

- `ticket-schema`
- `storage-migration`
- `workflow-state`
- `panel-actions`
- `orchestrator-routing`
- `backward-compatibility`
- `typed-tool-api`

Reviewer は diff だけでなく、CLI / typed Ticket tools / Panel / Orchestrator routing / local role-session claims / future relation metadata が、slug や two-axis state を authority として残していないことを確認する必要がある。

### 次の Intake action

上記 open questions に user/maintainer decision が入れば、Ticket を `implementation_ready` として `ready` にできる。未回答のまま進める場合は、先に design/spike として routing し、実装 Ticket とは分けるのが安全。

---
