# Ticket native WIP projection

## Purpose and authority

When a Worker explicitly runs in `wip` mode and its Ticket Feature is enabled, the Worker Host projects the enabled Ticket surface as native Worldspace objects. The projection is an adapter over the existing scoped Workspace Ticket API and typed Ticket tools. It does not add a Ticket store, resolver, audit path, state machine, readiness rule, queue path, or relation implementation.

The Host assigns the Ticket provider the root `tickets` namespace through `WipMountRegistry` and mounts:

```text
/tickets
/tickets/<ticket-reference>
```

There is no Feature Object or `/features/ticket` compatibility alias. A legacy path supplied as current input fails with rediscovery guidance; saved history containing that text is left unchanged.

`<ticket-reference>` is either a canonical `T-<digits>` key or an existing canonical internal alphanumeric ID. Only a direct child is accepted. Nested suffixes, Objective keys, unrelated prefixes, and hidden routes do not resolve.

Every call retains the Worker's scoped `WorkspaceClient`, current Ticket Feature access, and ordinary Tool permission policy. Discovery is not mutation authority. A direct call to an unpublished operation fails even when the caller knows its name. `deny` and interactive `ask` permission outcomes fail closed before Backend dispatch.

## Current operation inventory

The native surface is assembled from the tools actually enabled by `TicketFeatureAccess`; it is not a second broad Ticket catalog. The native operation delegates to the named ordinary Tool and validates the reconstructed input against that Tool's exact JSON Schema.

Collection interface `yoi.ticket/collection/v1`:

| Native operation | Existing Tool identity | Subject identity |
| --- | --- | --- |
| `query` | `QueryTicket` | Collection; preserves bounded filters, limit and cursor |
| `create` | `TicketCreate` | Collection; Backend assigns identity to an open case |
| `tag_list` | `WorkspaceTagList` | Workspace classification; bounded stable-ID pagination |
| `tag_create` | `WorkspaceTagCreate` | Display name and exact-retry operation key |
| `tag_rename` | `WorkspaceTagRename` | Stable tag ID, observed `expected_last_operation_key`, name and operation key |
| `tag_delete` | `WorkspaceTagDelete` | Stable tag ID, observed `expected_last_operation_key` and operation key |

Item interface `yoi.ticket/item/v1`:

| Native operation | Existing Tool identity | Non-subject inputs |
| --- | --- | --- |
| `read` | `ShowTicket` | `event_limit`, `event_cursor` |
| `edit` | `TicketEditItem` | title/body replacement and targets edit |
| `comment` | `TicketComment` | comment body |
| `case_update` | `TicketCaseUpdate` | typed close/reopen/correction action, explanation, item/state/nullable closure-key CAS and exact-retry key |
| `assignment_tracking_query` | `TicketAssignmentTrackingQuery` | optional retained `assignment_id` |
| `assignment_tracking_history` | `TicketAssignmentTrackingHistory` | `assignment_id`, bounded limit and event cursor |
| `assignment_tracking_update` | `TicketAssignmentTrackingUpdate` | current `assignment_id`, observed `expected_last_event_id`, operation key and complete self-reported handling |
| `orchestrator_assign` | `TicketOrchestratorAssign` | target principal, explicitly observed nullable `expected_assignment_id`, stable `operation_id`; explicit reissue uses a new operation, exact retry keeps the original |
| `orchestrator_clear` | `TicketOrchestratorClear` | observed `assignment_id`, stable `operation_id` |
| `assignments_query` | `TicketAssignmentsQuery` | bounded limit and `after_assignment_id` cursor for retained rows |
| `assignment_diagnostics` | `TicketAssignmentDiagnostics` | `assignment_id`, bounded limit, independent `after_input_event_id` / `after_start_operation_id` cursors |
| `assignment_answer` | `TicketAssignmentAnswer` | observed same `assignment_id` / `expected_last_event_id`, stable `operation_key`, bounded answer body |
| `tags_set` | `TicketTagsSet` | stable desired `tag_ids`, observed full `expected_tag_ids` set and operation key |
| `dependency_check` | `TicketDependencyCheck` | none |
| `record_relation` | `TicketRelationRecord` | relation kind, **target Ticket**, optional note |
| `remove_relation` | `TicketRelationRemove` | relation kind and **target Ticket** |
| `record_orchestration_plan` | `TicketOrchestrationPlanRecord` | kind, related Ticket, note and accepted-plan data |
| `query_orchestration_plans` | `TicketOrchestrationPlanQuery` | relation kind and bounded limit |

The subject `ticket`/`id` parameter is removed from every item descriptor and reconstructed exclusively from the resolved route. A caller cannot replace it through arguments. A relation target or related Ticket remains an explicit, distinct input and is resolved by the existing typed Backend.

Access profiles therefore expose different native interfaces:

- read-only/review: collection query/tag list; item read, assignment tracking query/history, retained assignments and sanitized diagnostics;
- workspace authoring/intake: query/create, read/edit/comment/case judgment, relation/classification operations, tracking reads, explicit Orchestrator Assign/reissue/clear and continuing answer;
- workflow/orchestration: query/read/comment/case judgment, dependency diagnostics, relation/plan/classification operations, tracking reads/self-report update, explicit Assign/reissue/clear and continuing answer;
- work-report: query/read/comment/case judgment/classification, assignment reads/diagnostics and continuing answer. Actual exposure is additive from manifest flags; authoring-enabled Coder configurations also expose delegation, subject to Backend authorization.

There are no current native ready/intake-ready/queue/six-state transition operations. Assign is delegation without a separate Queue; Worker starts are separately guarded and require the current observed `orchestrator_assignment_id` for Orchestrator calls. Assignment public operations call shared typed `WorkspaceClient::execute_server_operation` forwarding with actual Server DTOs; no direct HTTP/storage fallback or second ledger is introduced. Role assignment tools target Orchestrator responsibility only, not Worker execution/lifecycle; target principal is never the actor. Query first, preserve exact operation/input on retry, and reread after conflict/unknown outcome rather than automatically rekeying. Missing host support is a concrete authority error, never an alternate transport.

No native operation is inferred for a Tool that the Feature did not register. Enabled Ticket tools claimed by the native projection are removed from `/tools`; normal Tool mode is unchanged.

## Bounded reads and references

`query` uses the authoritative Workspace bounded query and cursor contract. Results include canonical `T-*` references and native item paths without embedding unbounded thread or evidence history. `read` uses the authoritative bounded detail projection for thread events, relations, Objective references, assignments, Merge Requests and evidence. Objective and Merge Request values remain references/data; this projection neither assumes their native routes nor grants their authority.

Backend 401/403 failures map to permission denial, 404 to missing objects, and deterministic 400/409/422 validation or workflow failures to invalid arguments when the typed operation fails before any successful mutation. Cancellation and interruption remain distinct. Post-mutation canonical-reference lookups are explicitly marked as unknown-outcome stages, so even a later deterministic HTTP status cannot misreport an already-committed write as a rejection. Transport, server, malformed response, and other post-dispatch failures retain unknown outcome, and mutating operations are not retried automatically.

Successful reads observe the authoritative item `content_digest`. Successful mutations advance a local observation generation even when the first call uses an internal ID whose canonical alias has not yet been learned. Once a read or mutation reveals the canonical key, the canonical/internal alias groups are merged and invalidated together. Relation mutations also invalidate the reported target Ticket because its inverse relation view changed. A stale validator is rejected by the common WIP Host before another Backend dispatch; the caller must rediscover/reinspect and explicitly decide whether to call again.

## Preserved Ticket contracts

The projection invokes the existing Tool and scoped Backend routes, so the following remain authoritative and are not reimplemented by WIP:

- item content digest and audited item/thread/state events;
- title/body/targets input validation;
- open/closed case judgments with item/state/closure-key CAS, typed reason and immutable close/correction/reopen history;
- stable classification tag IDs, tag operation CAS, full tag-set comparison and exact-retry receipts;
- immutable assignment identity and self-reported waiting/inprogress/done, observed event CAS and complete-payload exact replay;
- dependencies as diagnostics rather than ready/Queue or blanket launch gates;
- canonical `T-*`/internal-ID resolution and Workspace isolation;
- exact forward relation ownership, inverse views and relation target validation;
- orchestration-plan validation and bounded query behavior.

The ordinary JSON Schema is retained as a second validation boundary behind the WIP descriptor. Handling is never inferred from delivery, Worker or MR state. Waiting answers continue the same assignment; explicit done reissue creates a new row and preserves the prior result. Done does not close the case, release work, stop a Worker or require unassignment. Generic reasoned closure has no universal MR/human-approval gate; explicit user conditions and guarded MR integration remain separate. Exact retry retains the same operation key and full input; a receipt is not a fresh observation. On stale/current-assignment or CAS conflicts reread and reconcile; unknown outcomes must not be silently retried as a fresh operation. Optional fields omitted by the caller stay omitted, while an explicitly supplied `null` stays present as JSON `null`, so permission matching sees the same input shape as normal Tool mode. Defaults are applied only by the same ordinary typed Tool that applies them in Tool mode.
