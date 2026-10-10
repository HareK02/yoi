# Workspace Ticket classification tags

T-740 adds classification independently of Ticket state, close judgments, Orchestrator tracking, Worker lifecycle, repository grants, and MR evidence. T-739 owns case-state/close migration; T-743 owns the final Web management UI.

## Identity and names

`WorkspaceTag` contains `workspace_id`, immutable `id`, display `name`, `last_operation_key`, and `deleted`. Every operation resolves the ID within the route Workspace, never by display name or an unscoped numeric identity. A rename preserves links; a deleted tag is a tombstone, and recreating its name creates a new identity.

Names are trimmed with Rust Unicode whitespace trimming, then compared by Unicode lowercase. Display casing is retained. No Unicode canonical/compatibility normalization or locale-specific case folding is implied: canonically equivalent but differently encoded strings can remain distinct. Empty names, control characters, and names longer than 128 UTF-8 bytes are rejected. Active normalized names are unique per Workspace; deleted names are available for reuse. `ready`, `closed`, and other apparent control words are ordinary classification names.

Legacy `TicketMeta.labels` is not the classification authority and is not reinterpreted or removed by this change.

## Mutation and observation

- `GET /api/w/{workspace_id}/ticket-tags`: active tags ordered by stable ID; optional `after` ID and `limit` (bounded to 200). Continue with the last returned ID. A full page is not a promise of another page; continue until exhausted.
- `POST /api/w/{workspace_id}/ticket-tags`: `WorkspaceTagMutation` with `operation_key` and `action` (`kind: create | rename | delete`). Rename/delete require `tag_id` and the observed `expected_last_operation_key`; create/rename require `name`.
- `PUT /api/w/{workspace_id}/tickets/{id}/tags`: `TicketTagSetRequest` with `operation_key`, `expected_tag_ids`, and desired `tag_ids`. Add/remove multiple IDs by computing a desired set from the current detail/list/query projection. This explicit full-set comparison prevents silently overwriting another user's unrelated membership change. IDs are unique sets, not ordered lists for membership comparison; duplicate IDs are rejected. Exact replay retains the original array order as part of the original input. At most 100 active tags may be attached to one Ticket.

The tag's observed operation key compares the last committed change of that exact identity, including rename-back, before rename/delete. Membership compares the actual current active ID set, not a generic update number, timestamp, body digest, or state. An intervening change which restores exactly the same set is intentionally equivalent: the requested operation depends only on present membership, not on unseen membership history. An observed set containing a subsequently deleted ID is stale and fails, rather than implicitly dropping references.

SQLite immediate transactions serialize deletion and attachment. Delete atomically tombstones the tag and removes current membership; attachment rejects missing/deleted/foreign-Workspace identities. Neither outcome leaves an active dangling link. Foreign keys preserve Workspace/Ticket/tag identity integrity.

Operation keys belong to the Workspace classification mutation namespace. Authenticated identity is compared separately from mutable assignment/role context: reassignment does not make an identical receipt into another request, while the original full context remains in the audit. Exact replay compares the operation input, target, and authenticated source identity and returns the original stored response before stale/live checks; it does not repeat audit/event writes. Reusing a key for different input or identity conflicts. A replay is a receipt of the original result, not a fresh observation (its response can contain a subsequently renamed/deleted tag); reread detail/list before editing. Retry an uncertain request with its original key and input.

## Authorization and history

Browser and Worker operations use the same Backend tag store and existing Ticket Workspace authorization. The current shared Ticket policy permits authenticated Server users and authorized Runtime-forwarded Workers; tag management does not introduce an owner-only repository/secrets policy or a new membership/grant system. Runtime-forwarded sources are verified using existing source-proof checks, not trusted from arbitrary headers. Authenticated source attribution is retained in audit records; caller-supplied names/roles cannot grant authority. No local filesystem fallback or separate Worker tag namespace is provided.

Workspace mutations preserve an append-only audit with tag identity and display-name snapshots. Ticket membership changes preserve typed thread events with before/after identity/name snapshots. Deletion preserves tombstones and prior snapshots, so history still means the names and identities observed at the time, not today's display names. Historical events are not rewritten on rename or delete. An unchanged membership set records a receipt without claiming a `tags_changed` thread event. Explicit deletion of the entire Workspace purges its tag catalog, memberships, and receipts through the existing domain purge inventory.

Classification does not edit the Ticket's editable item content digest, state, close reason, resolution, assignment, or grants; it does not enter routing notifications or start/stop/delegate Workers. Close reasons remain a separate judgment contract even if a UI presents both using similar visual chips.

## Projections, filtering, and consumers

Domain `Ticket`/`TicketSummary`, human list/detail/query projections, `ShowTicket`/`QueryTicket`, and typed client responses expose `tags: WorkspaceTag[]`. Deleted tags are absent from current projections but remain in receipts/audit/history.

Ticket query/search takes stable IDs:

- `tag_ids`: all IDs must be attached (AND).
- `tag_ids_any`: at least one ID must be attached (OR), unless empty.
- `tag_ids_none`: none may be attached (NOT).

Nonempty groups combine with AND and with existing text/state/objective/relation/evidence filters. Unknown/deleted/foreign IDs never resolve to another tag: required IDs yield no matches, excluded IDs exclude nothing. Filters are bounded to 100 IDs per group. SQL filtering occurs before bounded candidate selection, then authoritative projections recheck membership. Pagination is not a database snapshot across requests; query-bound cursors cannot be reused with changed filters.

Tools expose Workspace tag creation/list/rename/delete and Ticket tag-set editing. The WIP native operations use the same tool/client boundary and permissions. OpenAPI is generated from typed ServerApi/domain schemas; browser Ticket types are generated from the canonical OpenAPI document. T-743 should use these public operations and explicit stable IDs, not invent Web-only tag state or depend on tag names for execution policy.

## Shared integration with T-739

Both branches touch Ticket operation enums, summary/detail construction, ServerApi query/detail contracts, tool projections, generated OpenAPI/TypeScript, and Ticket SQLite schema validation. Merge must compose migrations and current schema, not merely pick a higher schema version or overwrite one branch's baseline. Preserve tag tables, receipts, audit rows, tombstones and memberships through case-state migration. Re-run migration/equivalence tests and regenerate public contracts after composition. T-740 itself is buildable on the existing case-state model and does not import T-739's state/close migration.
