---
title: 'Panel Queue action should allow ready Tickets whose blockers are already queued or in progress'
state: 'done'
created_at: '2026-06-20T05:18:00Z'
updated_at: '2026-06-20T05:19:32Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['ticket', 'panel', 'queue', 'dependency', 'blocker', 'orchestrator']
---

## Background

Panel ViewModel already treats a ready Ticket as queueable when all blocking relations point at Tickets that are already `queued` or `inprogress`. That matches the intended workflow: once prerequisite work is queued/accepted by Orchestrator, dependent ready Tickets should also be queueable so the human does not need to return after every prerequisite completes.

However, pressing Enter in Panel still calls the backend `queue_ready` gate, and that backend rejected any unresolved relation blocker regardless of blocker state. This caused a `ticket conflict` even though the UI correctly showed Queue as available.

Concrete example:

- `00001KVHKWNQS` depends on `00001KVHKWNQA`.
- `00001KVHKWNQA` is already `inprogress`.
- Panel shows `00001KVHKWNQS` as queueable.
- Enter/Queue fails with backend ticket conflict.

## Requirements

- Backend `queue_ready` must match the Panel gating rule.
- A ready Ticket may transition `ready -> queued` when every relation blocker is already `queued` or `inprogress`.
- A ready Ticket must still be blocked when any relation blocker is `planning` or `ready`.
- This change applies to queueing only.
  - `queued -> inprogress` acceptance remains blocked while unresolved dependency/blocker relations exist; Orchestrator must preserve execution order after queueing.
- Existing Panel display behavior should remain aligned with backend behavior.

## Implementation summary

- Added backend helper `relation_blocker_allows_queue`.
- Updated `LocalTicketBackend::queue_ready` to filter relation blockers by that helper before rejecting.
- Kept `queued -> inprogress` relation blocker validation unchanged.
- Added backend test coverage for ready Tickets with queued/inprogress blockers.
- Re-ran existing Panel ViewModel test that already asserted ready+queued dependency appears queueable.

## Acceptance criteria

- `ready` Ticket with `depends_on` target in `queued` state can be queued.
- `ready` Ticket with incoming `blocks` blocker in `inprogress` state can be queued.
- `ready` Ticket with dependency/blocker still in `planning` remains rejected.
- Panel and backend queue behavior are consistent.
- Orchestrator acceptance ordering remains guarded by existing `queued -> inprogress` relation check.

## Validation

- `cargo test -p ticket queue_gate --lib`
- `cargo test -p tui workspace_panel_allows_ready_ticket_when_relation_prerequisite_is_queued --lib`
- `cargo check -p ticket -p tui`
- `cargo fmt`
- `git diff --check`
