# Objective native WIP projection

## Purpose and authority

When a Worker explicitly runs in `wip` mode and the Objective Feature is enabled, the Worker Host projects Objectives as native Worldspace objects. The projection is an adapter over the existing scoped Workspace API. It does not introduce another Objective store, state machine, reference resolver, or audit path.

The Host allocates `/features/objective` to the Feature through `WipMountRegistry`. The Objective Feature then mounts:

```text
/features/objective/objectives
/features/objective/objectives/<objective-reference>
```

`<objective-reference>` is either a canonical `O-<digits>` key or an existing canonical internal alphanumeric ID. Only one direct child segment is accepted. Nested suffixes, Ticket keys, and unrelated or hidden prefixes do not resolve as Objective objects.

The Backend remains authoritative for existence, `O-*`/internal-ID resolution, Workspace scope, validation, audit events, revision generation, pagination, body/event bounds, and linked-Ticket validation. A discovered route is not independent authority; every operation still uses the Worker's current `WorkspaceClient`.

## Current operation inventory

The collection interface is `yoi.objective/collection/v1`:

| Native operation | Existing Tool permission identity | Parameters |
| --- | --- | --- |
| `query` | `QueryObjective` | `query`, `states`, `linked_ticket_id`, `updated_after`, `updated_before`, `sort`, `limit`, `cursor` |
| `create` | `ObjectiveCreate` | `title`, `body_md`, `state`, `linked_tickets` |

The item interface is `yoi.objective/item/v1`:

| Native operation | Existing Tool permission identity | Parameters |
| --- | --- | --- |
| `read` | `ShowObjective` | `event_limit`, `event_cursor` |
| `edit` | `ObjectiveEdit` | `title`, `old_string`, `new_string`, `replace_all` |
| `set_state` | `ObjectiveSetState` | `state` |
| `link_ticket` | `ObjectiveLinkTicket` | `ticket_id` |
| `unlink_ticket` | `ObjectiveUnlinkTicket` | `ticket_id` |

The item operations intentionally do not accept an Objective `id`. The handler is created from the resolved item route and reconstructs the equivalent ordinary Tool permission arguments with that route-bound identity. Descriptor validation rejects a caller attempting to supply a second Objective target.

`deny` and `ask` permission results fail closed before Backend dispatch. Backend 401/403 map to permission denial, 404 to missing target, and deterministic client validation failures to invalid arguments. Transport, server, or response-projection failures after dispatch retain the common unknown-outcome contract; mutating operations must not be retried automatically when the outcome is unknown.

## Bounded reads and references

`query` retains the ordinary Worker limit of 1–100 and returns stable snippets, canonical `O-*` references, linked `T-*` references, `next_cursor`, and `has_more`. Each result also includes its canonical item path. It does not expand Objective bodies, complete events, or Ticket records.

`read` retains the ordinary Worker event limit of 1–50 and projects the Backend-bounded body, `body_truncated`, authoritative `revision`, linked-Ticket summaries, bounded events, and event pagination. Internal Objective, Ticket, and event-storage IDs are removed from model-visible results.

`create` returns the authoritative Objective detail plus its canonical item path. Mutations return the authoritative post-operation detail. The revision observed in a successful read or mutation updates the item's object validator. The common Host validator contract rejects stale object/interface calls before dispatch, and the call response carries the refreshed route validator.

Linked Ticket references remain strings passed to the existing Objective Backend operation. Their presence does not publish a Ticket object, grant Ticket read or mutation authority, or require a Ticket-native WIP projection.

## Compatibility and Tool mode

The native collection/item pair claims all seven Objective compatibility capabilities in WIP mode. Therefore `/tools/QueryObjective`, `/tools/ShowObjective`, and `/tools/Objective*` are not published alongside the native routes. Unrelated enabled tools keep their compatibility projections.

Normal `tools` mode is unchanged: it still exposes the same seven Objective tools with their existing names and input schemas. Both surfaces share the same `WorkspaceHttpObjectiveBackend` and scoped Backend endpoints.
