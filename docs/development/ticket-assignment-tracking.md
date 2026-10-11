# Ticket assignment tracking contract (T-741)

## Authority and identities

Ticket component schema 11 owns assignment identities, current pointers, assignment audit/receipts, and the self-reported handling data. The historical physical names `ticket_worker_assignments`, `ticket_current_worker_assignments`, `ticket_worker_assignment_events`, and `ticket_assignment_operations` are retained; they are not another Workspace assignment authority. Workspace schema 92 composes these Ticket-owned definitions and keeps host principal/resource constraints.

An immutable `assignment_id` identifies one delegation. Progress updates modify that assignment row; a new Assign/reissue operation creates another row even for the same principal. An exact retry returns the saved result, not a new delegation or a fresh observation.

The Orchestrator principal is `workspace_agent:workspace-orchestrator`. Backend resolves its current registered Worker. Worker stop, idle, restore, replacement, MR changes and notification delivery do not update handling status or create assignment rows.

Worker creation reservations, attachment/grant recovery, actual unfinished work, and identity-specific work release remain operational responsibilities. Handling `done` is not work release, Worker termination, resource cleanup, MR approval/integration, or a Ticket close judgment.

## Self-report and concurrency

The handling payload contains `status` (`waiting`, `inprogress`, `done`) and nullable `waiting_reason`, `progress`, `next_action`, and `result`. Assign records initial waiting/initial judgment pending together with the current pointer, audit event, and receipt in one transaction.

The update input is `assignment_id`, `expected_last_event_id`, `operation_key`, and the complete handling payload. The observed event belongs to that exact delegation; comparing status alone would miss concurrent changes. No ordered status progression, MR or approval requirement applies. Direct waiting-to-done is valid.

Backend constructs authenticated actor/source context; model input cannot supply role, actor, Runtime or Worker authority. The mutation transaction validates current singleton ownership and the Runtime binding actually used to verify the request. Full input/source must match an existing receipt. The receipt check distinguishes an old operation retry from a new update to a released/replaced delegation; only the latter requires current assignment/event CAS. Worker replacement does not let the old sender replay operations.

The public Orchestrator Assign/clear paths also bind receipts to Backend-authenticated Runtime/Worker/agent/binding context or the authenticated account/user pair, separately from readable Assign/audit actor text. Runtime calls fence singleton and binding inside the write transaction even for retries. Account authoring retains its authenticated Workspace permission boundary and does not require a Runtime identity. Migration preserves old receipts without inventing a historical binding proof.

Close/reopen does not change handling status. Current Orchestrator result recording remains authorized after close without requiring an active-worker assignment or restoring old execution grants. Reopen by itself creates neither a new delegation nor old Worker execution rights.

## Public surfaces

Canonical Workspace endpoints:

- `GET /workspaces/{workspace_id}/tickets/{ticket_id}/assignment-tracking`: current Orchestrator assignment projection. Optional `assignment_id` selects a retained assignment, including its last handling and independent release/replacement audit.
- `POST` on the same path: self-report update with event CAS and exact retry key.
- `GET .../assignment-tracking/history`: assignment-scoped handling audit, with `assignment_id`, optional `after_event_id`, and `limit` (1–100). The returned `next_event_id` continues that history.

Normal tools and route-bound WIP operations expose `TicketAssignmentTrackingQuery`, `TicketAssignmentTrackingHistory`, and `TicketAssignmentTrackingUpdate`. T-743's public handoff adds `TicketAssignmentsQuery` for bounded retained Orchestrator rows (`GET .../assignment-tracking/assignments`, `after_assignment_id`, limit 1–100, `next_assignment_id`), `TicketAssignmentDiagnostics` for sanitized independent delivery/start pages (`GET .../assignment-diagnostics`, assignment ID, separate input/start cursors), and `TicketAssignmentAnswer` (`POST .../assignment-answer`). Answer uses the existing authenticated operation receipts with assignment/event CAS and bounded body; it continues waiting/inprogress on the same row and does not change handling. Done requires explicit reissue instead.

`TicketOrchestratorAssign` / `TicketOrchestratorClear` use the existing role-assignment routes with a target principal, observed assignment identity and stable `operation_id`; explicit nullable `expected_assignment_id` is mandatory in Assign Tool/WIP input. These tools target responsibility, not Worker execution/lifecycle, and never derive the actor from model role text. Query first; retry exact full input with the original identity, never automatically generate a fresh key on stale/unknown outcome. Typed forwarding is shared `WorkspaceClient::execute_server_operation`; it does not introduce another ledger or direct HTTP/storage fallback. Read-only/review access includes retained rows and diagnostics, not Assign/clear/answer/self-report mutation. Ticket query/detail and model projections include current assignment tracking; `QueryTicket` also exposes exact current `assigned_principal` and self-reported `handling_status` filters. Read access does not grant mutation authority; Backend checks every update.

Read a fresh projection before a new update. Do not treat a replayed receipt's last-event identity as the latest observed value. Retained-assignment query exposes Assign attribution and release/replacement independently; the history endpoint pages initial/migrated handling and subsequent self-reports, not delivery acknowledgements.

Update identities and operation keys are nonempty, control-character-free and bounded to 1024 bytes. Each optional handling text is bounded to 16384 UTF-8 bytes and cannot contain NUL. History cursors must belong to the same Workspace, Ticket and assignment. Read-only tool access includes query/history, never update; enabling a tool is not Backend authorization.

## Migration and downstream handoff

Ticket 10→11 and Workspace 91→92 append migrations; applied migrations are not rewritten. Original workflow snapshots, Queue assignment fences, assignment identities/audit/receipts and subsequent case decisions are retained and reconciled. Current fenced queued/in-progress delegation can be retained with its migration evidence. Historical, ambiguous, or superseded relationships retain original data plus diagnostics with unknown handling (`tracking: null`); neither successful done nor a fresh execution request is inferred.

For retained current relationships with unknown handling, an existing matching Assign audit event can serve as the observed `last_event_id` baseline for the first explicit self-report, including after close. This is not a new handling event or an initial waiting value. The first self-report replaces that baseline with its own event; concurrent updates then fail normal CAS. If original Assign audit is missing, keep the diagnostic rather than inventing an event. Handling-history cursors remain identities returned by that history, not this Assign baseline.

T-742 consumes this authority for delivery/acceptance/reconsideration cursors and startup/create/restore/idle scans. It must distinguish committed new Assign events from legacy unknown relationships, and bind actual acceptance/start reservations to current assignment identity. It owns replacement of the old Queue/state routing predicate and unfinished-dependency start refusals. Waiting alone is not a reason for repeated delivery, and updating one's own progress must not become an input busy loop.

T-743 owns the final UI/Prompt operation model. Classification, Ticket case state, handling, delivery/consideration position, and actual Worker/grant lifecycle remain separate. This contract does not implement live DB migration or update the running environment.
