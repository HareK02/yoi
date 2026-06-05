# Implementation report: ticket-orchestrator-routing

## Summary

Added the Ticket Orchestrator Routing workflow and lightly updated workflow development documentation.

The workflow defines Orchestrator as the routing authority after Intake materializes a Ticket. It requires Orchestrator to inspect Ticket fields/events/artifacts and relevant repository/Pod state explicitly, classify the next action, and record the routing decision back to the Ticket.

## Changed files

- `.yoi/workflow/ticket-orchestrator-routing.md`
- `docs/development/workflows.md`

## Routing classifications

The workflow defines the following routing outcomes:

- `requirements_sync_needed`
- `preflight_needed`
- `spike_needed`
- `implementation_ready`
- `review_needed`
- `blocked_action_required`
- `close_ready`
- `defer_pending`
- `closed_or_noop`

Each classification includes conditions and the expected next action.

## Key behavior

- Routing decisions are based on Ticket fields/events/artifacts plus explicitly inspected repository/Pod state, not hidden conversation state.
- `TicketComment` is used to record routing decisions.
- `implementation_ready` requires a concise `IntentPacket` before connecting to `multi-agent-workflow`.
- `preflight_needed` Tickets are not sent directly to coder Pods.
- `spike_needed` uses read-only investigation only after authorization.
- `review_needed` requires review/validation evidence before merge-ready handling.
- `close_ready` still requires close authority and a resolution.

## Non-goals preserved

The workflow explicitly does not introduce:

- unattended scheduler;
- LeaseStore / queue persistence;
- automatic Pod spawning policy;
- action-required dashboard UI;
- TicketUpdate tool;
- external tracker integration;
- arbitrary filesystem writes.

## Review status

External sibling reviewer approved with no blockers and no required non-blockers.

Reviewer follow-up note:

- A future pure classifier/test fixture may be useful after repeated routing decisions stabilize, but it is explicitly out of scope for this ticket.

## Validation

Validation passed:

- `git diff --check`
- `./tickets.sh doctor`
- targeted grep found no `WorkItem` / old system-name wording in the new workflow.

No code/package changes were made, so cargo/nix validation was not required.
