<!-- event: migration author: tickets.sh-migration at: 2026-05-27T00:00:17Z -->

## Migrated

Migrated from tickets/tui-spawned-pod-panel.md. No legacy review file was present at migration time.

---

<!-- event: decision author: hare at: 2026-06-05T04:03:38Z -->

## Decision

Decision: deprioritize this ticket for the current multi-agent system direction.

Current need is not a TUI panel for spawned Pods. The priority is Ticket-driven intake/routing: making Tickets a code-facing durable orchestration record, then exposing Ticket operations to Intake/Orchestrator through a typed backend/tool surface.

This ticket is not closed as technically invalid; it is moved out of the active multi-agent implementation path. Revisit only if direct child Pod visibility/attach UI becomes a concrete UX requirement.


---

<!-- event: state_changed author: hare at: 2026-06-07T03:14:39Z from: intake to: done reason: closed field: workflow_state -->

## State changed

Ticket closed; workflow_state set to done.


---

<!-- event: close author: hare at: 2026-06-07T03:14:39Z status: closed -->

## Closed

Closed as intentionally not planned.

The old migrated spawned-Pod panel idea has been superseded by the workspace panel, Pod list/open/attach behavior, Ticket role launching, and the local role session registry. The remaining direction is not to revive this standalone spawned-child panel ticket. Future panel work should be tracked through the newer workspace panel / orchestration tickets.

---
