# Assign-driven Orchestrator delivery and start admission (T-742)

## Separate authorities

[Ticket assignment tracking](ticket-assignment-tracking.md) remains the sole delegation/handling authority (Ticket component schema 11). Workspace schema 93 adds **operational** input origins, delivery reservations, exact rendered requests, acceptance acknowledgements, and assignment/source-bound start fences. These tables do not create another assignment identity or handling status.

Assign commits the current pointer, initial waiting, assignment event and source-bound operation receipt together. Its committed tracking-history insertion records an initial input origin in that same transaction. Delivery runs only after commit. Runtime acceptance, running/idle/stop, MR changes, and delivery acknowledgement never update `waiting`/`inprogress`/`done`.

A delivery acknowledgement means the Runtime accepted the immutable Notify request. It is not an assertion that the model considered the input, satisfied the Ticket, or started execution. Runtime notification/activation receipts provide the corresponding durable session boundary. Self-reported progress/result remains a separate explicit mutation.

## Common input scan and delivery

Mutation notification, Backend startup, Orchestrator create (including already-online), accepted restore, replacement, and running→idle all use `list_pending_ticket_assignment_deliveries`. There is no parallel Queue/open-state routing predicate. Openness is a new-work safety boundary, not delegation intent.

The scan selects current `workspace_agent:workspace-orchestrator` assignments with actionable handling and an explicit committed input origin. It excludes released identities, unknown legacy relationships, done/history and closed cases. External content/answer events after the origin are new inputs; handling self-reports and case/routing history are not. Waiting alone is never a resend reason. Close releases operational work; reopen alone does not revive the old delegation. Explicit reissue creates a new assignment and initial input.

Schema 92→93 preserves original assignment/event/Queue evidence. Native Assign history establishes its input origin; migrated tracking contributes an input only where its already-reconciled assignment-specific Queue fence is present. Ambiguous/historical rows remain diagnostic data, not fresh requests. Earlier migrations and Ticket schema 11 are unchanged.

Delivery order:

1. Resolve the currently registered online Worker and active Runtime binding from Backend authority (a stopped Worker is not automatically restored).
2. Reserve the exact `assignment_id`/`input_event_id` under IMMEDIATE, rechecking current delegation, singleton and binding.
3. Render using the reservation's metadata, including waiting reason and next action. Persist the **entire rendered content**, not just its metadata or a title fingerprint.
4. Send the saved content with the saved `request_id`, outside the transaction. Response loss retains the same request/payload. Concurrent reservations converge on the same identity, and Runtime request receipts deduplicate actual acceptance.
5. Acknowledge accepted delivery under another IMMEDIATE transaction after rechecking current authority. The acknowledgement never changes handling.

Process-local attention locks prevent concurrent/reentrant send loops; they are not backlog authority. Losing that cache does not lose a delegation or resend acknowledged input. Replacement does not create assignment rows; only pending, unacknowledged inputs are recovered. Accepted history is not replayed into new work merely because a Worker was exchanged.

## Public start/action handoff for T-743

Existing canonical tracking query/history/update endpoints, Tools/WIP, Ticket query/detail, and retained assignment projections remain available; see the tracking document. Model-visible notices include the committed delegation/input identity and bounded waiting/next-action context. The Server attention template is updated here; final Web/TUI/all-Prompt operation alignment belongs to T-743.

Start request contracts now carry optional `orchestrator_assignment_id`:

- `CreateWorkspaceWorkerRequest.ticket_assignment` (including Worker control spawn).
- `RuntimeWorkerSpawnRequest.ticket_assignment`.
- `SetTicketRoleAssignmentRequest` for an existing Worker.
- `SpawnTicketWorker` and generic `WorkerSpawn` tool input/lifecycle forwarding.

An authenticated current Orchestrator **must** supply its observed delegation identity. Backend checks it with the actual `ServerRequestContext.runtime_source.authentication_binding_id`; it never derives the source from model role text or reacquires another binding as authentication proof. Missing/stale/cleared/reissued identities and obsolete Workers cannot admit work. Manual/generic authorized starts omit the delegation field and do not implicitly assign the Orchestrator. New starts and existing Worker assignments share the same reservation/start fence.

The fenced reservation extends the existing Ticket operation receipt and Worker allocation/recovery identity. Reserve/retry, bind and finalization validate the saved assignment/source under IMMEDIATE. Recovery/pre-dispatch checks use the same fence. Clear/reassign can invalidate a reservation but do not stop, transfer or clean up already-running work. Unknown external outcomes keep their reservation/resource fences for explicit reconciliation rather than permitting a second start. Runtime I/O is never inside a database transaction.

Unfinished dependencies remain query/relation diagnostics and do not disable manual/Orchestrator new starts or existing Worker assignment. They are never implicitly started. Case closure, authenticated permissions, actual unfinished work/claims, resource/grant conflicts, target containment and recovery remain independent guards. Repository/Workdir-free work, read-only work and caller-selected published Profiles are preserved.

Action projection permits delegation independently of existing Worker occupancy; actual Worker start eligibility still excludes unfinished Worker work and closed cases. `can_start_manual_worker` no longer includes an unresolved-dependency predicate. `can_assign_orchestrator` permits an explicit fresh Assign/reissue operation even when a current row exists; callers can supply the observed expected assignment for CAS. `can_unassign_orchestrator` uses the retained current pointer, not active-work or case-state filtering. These authoring actions are not Worker transfer. Final browser controls must use these Backend decisions rather than recreating old phase/dependency gates.

Delivery diagnostics are available through the typed Store scan/reservation/ack records and structured `ticket_assignment_delivery_pending` warning (`assignment_id`, `input_event_id`, `delivery_id`, recipient and error category, never content). Existing `ticket_notification_delivery_failed` remains for recipient failures. No browser acknowledgement/status-edit endpoint is introduced: successful delivery is not a user handling update. T-743 can consume the operational query independently of handling without inventing a third status in tracking.

## DB caller/transaction handoff for T-748 and protocol boundaries

Store callers are in `store.rs`; operational SQL/mutations are in `assignment_delivery.rs`. The public Store methods are `list_pending_ticket_assignment_deliveries`, `reserve_ticket_assignment_delivery`, `prepare_ticket_assignment_delivery_content`, `ack_ticket_assignment_delivery`, `reserve_ticket_assignment_operation_for_orchestrator`, and `validate_ticket_assignment_operation_fence`. Existing bind/finalization writes call the saved-operation guard inside their own IMMEDIATE transaction.

Operational tables: `ticket_assignment_delivery_origins`, `ticket_assignment_deliveries`, `ticket_assignment_delivery_requests`, `ticket_assignment_delivery_acknowledgements`, `ticket_assignment_operation_fences`. Workspace explicit purge inventory includes their scoped rows and temporarily suspends/restores their immutable-delete guards in the existing guarded purge transaction. Ordinary operations cannot delete or rewrite receipts.

T-741 set/clear/update/history remain unchanged in transaction ownership: actual source binding/singleton checks, receipt comparison, current/event CAS, audit and result must stay together. A future DB blocking adapter must move the whole synchronous transaction as one unit, not split its queries around awaits. Server Runtime method admission and `ServerRuntimeMutationSourceContext.authentication_binding_id` are not changed. T-739 close/reopen and T-715 identity-specific work releases remain execution boundaries.

## Validation limits

Local Store tests, authenticated HTTP tests, real embedded Controller/session tests, and loopback remote Runtime HTTP tests prove persistence, concurrency, source/assignment fencing, payload replay, resource contracts and activation dedupe. They do not prove model judgment quality or final Web/TUI/all-Prompt behavior (T-743/T-744). No live DB migration, environment update, target integration, Ticket conclusion or Runtime Worker/Workdir cleanup is part of this handoff.
