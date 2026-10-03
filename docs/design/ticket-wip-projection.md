# Ticket native WIP projection

## Purpose and authority

When a Worker explicitly runs in `wip` mode and its Ticket Feature is enabled, the Worker Host projects the enabled Ticket surface as native Worldspace objects. The projection is an adapter over the existing scoped Workspace Ticket API and typed Ticket tools. It does not add a Ticket store, resolver, audit path, state machine, readiness rule, queue path, or relation implementation.

The Host allocates `/features/ticket` and the Feature mounts:

```text
/features/ticket/tickets
/features/ticket/tickets/<ticket-reference>
```

`<ticket-reference>` is either a canonical `T-<digits>` key or an existing canonical internal alphanumeric ID. Only a direct child is accepted. Nested suffixes, Objective keys, unrelated prefixes, and hidden routes do not resolve.

Every call retains the Worker's scoped `WorkspaceClient`, current Ticket Feature access, and ordinary Tool permission policy. Discovery is not mutation authority. A direct call to an unpublished operation fails even when the caller knows its name. `deny` and interactive `ask` permission outcomes fail closed before Backend dispatch.

## Current operation inventory

The native surface is assembled from the tools actually enabled by `TicketFeatureAccess`; it is not a second broad Ticket catalog. The native operation delegates to the named ordinary Tool and validates the reconstructed input against that Tool's exact JSON Schema.

Collection interface `yoi.ticket/collection/v1`:

| Native operation | Existing Tool identity | Subject identity |
| --- | --- | --- |
| `query` | `QueryTicket` | Collection; preserves bounded filters, limit and cursor |
| `create` | `TicketCreate` | Collection; Backend assigns identity |

Item interface `yoi.ticket/item/v1`:

| Native operation | Existing Tool identity | Non-subject inputs |
| --- | --- | --- |
| `read` | `ShowTicket` | `event_limit`, `event_cursor` |
| `edit` | `TicketEditItem` | title/body replacement and targets edit |
| `comment` | `TicketComment` | comment body |
| `mark_ready` | `TicketMarkReady` | optional reason |
| `intake_ready` | `TicketIntakeReady` | intake summary and optional reason |
| `queue` | `TicketQueue` | none |
| `transition` | `TicketWorkflowState` | expected/target state, reason and body |
| `close` | `TicketClose` | resolution |
| `dependency_check` | `TicketDependencyCheck` | none |
| `record_relation` | `TicketRelationRecord` | relation kind, **target Ticket**, optional note |
| `remove_relation` | `TicketRelationRemove` | relation kind and **target Ticket** |
| `record_orchestration_plan` | `TicketOrchestrationPlanRecord` | kind, related Ticket, note and accepted-plan data |
| `query_orchestration_plans` | `TicketOrchestrationPlanQuery` | relation kind and bounded limit |

The subject `ticket`/`id` parameter is removed from every item descriptor and reconstructed exclusively from the resolved route. A caller cannot replace it through arguments. A relation target or related Ticket remains an explicit, distinct input and is resolved by the existing typed Backend.

Access profiles therefore expose different native interfaces:

- read-only/review: collection `query`; item `read`;
- workspace authoring: query/create plus read/edit/comment/ready/queue/close/relation operations;
- intake: the authoring surface plus `intake_ready`;
- workflow/orchestration: query/read/comment, workflow transition/close/dependency checks, relation operations and orchestration-plan operations;
- work-report: query/read/comment only.

No native operation is inferred for a Tool that the Feature did not register. Enabled Ticket tools claimed by the native projection are removed from `/tools`; normal Tool mode is unchanged.

## Bounded reads and references

`query` uses the authoritative Workspace bounded query and cursor contract. Results include canonical `T-*` references and native item paths without embedding unbounded thread or evidence history. `read` uses the authoritative bounded detail projection for thread events, relations, Objective references, assignments, Merge Requests and evidence. Objective and Merge Request values remain references/data; this projection neither assumes their native routes nor grants their authority.

Backend 401/403 failures map to permission denial, 404 to missing objects, and deterministic 400/409/422 validation or workflow failures to invalid arguments when the typed operation fails before any successful mutation. Cancellation and interruption remain distinct. Post-mutation canonical-reference lookups are explicitly marked as unknown-outcome stages, so even a later deterministic HTTP status cannot misreport an already-committed write as a rejection. Transport, server, malformed response, and other post-dispatch failures retain unknown outcome, and mutating operations are not retried automatically.

Successful reads observe the authoritative item revision. Successful mutations advance a local observation generation even when the first call uses an internal ID whose canonical alias has not yet been learned. Once a read or mutation reveals the canonical key, the canonical/internal alias groups are merged and invalidated together. Relation mutations also invalidate the reported target Ticket, while queue results invalidate every reported queued Ticket, because those objects' incoming relations or workflow states changed. A stale validator is rejected by the common WIP Host before another Backend dispatch; the caller must rediscover/reinspect and explicitly decide whether to call again.

## Preserved Ticket contracts

The projection invokes the existing Tool and scoped Backend routes, so the following remain authoritative and are not reimplemented by WIP:

- item revision and audited item/thread/state events;
- title/body/targets input validation;
- planning-to-ready target validation and write-target requirement;
- queue dependency traversal, planning-dependency rejection and cycle detection;
- expected-state workflow transitions and close semantics;
- canonical `T-*`/internal-ID resolution and Workspace isolation;
- exact forward relation ownership, inverse views and relation target validation;
- orchestration-plan validation and bounded query behavior.

The ordinary JSON Schema is retained as a second validation boundary behind the WIP descriptor. Optional fields omitted by the caller stay omitted, while an explicitly supplied `null` stays present as JSON `null`, so permission matching sees the same input shape as normal Tool mode. Defaults are applied only by the same ordinary typed Tool that applies them in Tool mode.
