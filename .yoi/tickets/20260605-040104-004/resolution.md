Ticket Orchestrator Routing Workflow is complete.

Implementation:

- commit: `af17f8b workflow: add ticket orchestrator routing`

Summary:

- Added `.yoi/workflow/ticket-orchestrator-routing.md` as a user-invocable/model-invocable workflow.
- Updated `docs/development/workflows.md` to include Orchestrator routing from Tickets to next workflow/action.
- Defined routing classifications:
  - `requirements_sync_needed`
  - `preflight_needed`
  - `spike_needed`
  - `implementation_ready`
  - `review_needed`
  - `blocked_action_required`
  - `close_ready`
  - `defer_pending`
  - `closed_or_noop`
- Defined conditions and actions for each classification.
- Required routing decisions to be recorded through `TicketComment`.
- Required `IntentPacket` creation before connecting implementation-ready Tickets to `multi-agent-workflow`.
- Kept preflight-needed Tickets from being sent directly to coder Pods.
- Preserved non-goals: no scheduler, no lease/queue persistence, no automatic Pod spawning policy, no TUI dashboard, no TicketUpdate tool, no external tracker integration, and no arbitrary filesystem writes.

Review:

- External sibling reviewer approved with no blockers and no required non-blockers.
- Follow-up note: after repeated routing decisions stabilize, a pure classifier/test fixture may be useful, but it is out of scope for this workflow ticket.

Validation passed:

- `git diff --check`
- `./tickets.sh doctor`
- targeted grep found no `WorkItem` / old system-name wording in the new workflow.

No code/package changes were made, so cargo/nix validation was not required.
