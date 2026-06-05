<!-- event: create author: tickets.sh at: 2026-06-05T04:01:04Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T06:37:59Z -->

## Plan

Preflight result: `implementation-ready` as a workflow/resource implementation.

Scope chosen for this ticket:

- Add a project workflow that makes Orchestrator routing explicit and repeatable.
- Define routing classifications, required evidence, TicketComment decision format, and IntentPacket format.
- Connect routing outcomes to existing workflows: Ticket Intake, Ticket Preflight, Multi-agent Worktree Workflow, review, blocked/human action, and close-ready handling.
- Update workflow development docs to include Orchestrator routing.

Non-goals preserved:

- No code classifier in this ticket.
- No scheduler/lease/queue persistence.
- No automatic Pod spawning policy.
- No TUI/action-required dashboard.
- No TicketUpdate tool.
- No external tracker integration.

The routing workflow is intentionally testable as a decision table/checklist now; a pure Rust classifier can be split later if the workflow proves stable.


---

<!-- event: review author: hare at: 2026-06-05T06:41:33Z status: approve -->

## Review: approve

# Review: ticket-orchestrator-routing

## 1. Result: approve

Approve. I found no blockers.

## 2. Summary

The new `ticket-orchestrator-routing` workflow satisfies the ticket scope as a workflow/resource change. It defines Orchestrator-owned routing responsibilities, keeps routing authority tied to Ticket records plus explicitly inspected repository/Pod state, and avoids introducing scheduler, lease, queue, dashboard, automatic spawning, or arbitrary filesystem-write behavior.

The implementation fits the current multi-agent direction: routing produces a recorded decision and, for implementation-ready Tickets, an `IntentPacket` that is handed to `multi-agent-workflow` rather than directly spawning coder Pods.

## 3. Requirement assessment

- **Ticket terminology:** Pass. The new workflow and docs use `Ticket` terminology and do not revive `WorkItem` / old-system wording.
- **Orchestrator responsibilities:** Pass. The workflow requires reading the Ticket body/thread/artifacts/status, inspecting relevant repository state and visible Pod state, classifying the next action, recording a routing decision through `TicketComment`, and creating an `IntentPacket` only for implementation-ready Tickets.
- **Non-goals:** Pass. The workflow explicitly excludes unattended scheduling, automatic Pod spawning, lease/queue persistence, TUI dashboard/action dashboard behavior, TicketUpdate mutation, external tracker integration, and arbitrary filesystem writes.
- **Routing classifications:** Pass. The workflow covers `requirements_sync_needed`, `preflight_needed`, `spike_needed`, `implementation_ready`, `review_needed`, `blocked_action_required`, `close_ready`, `defer_pending`, and `closed_or_noop`.
- **Conditions and actions:** Pass. Each classification has usable conditions and required next actions, including when to route back to intake/preflight, when to request spike work, when to review, when to block for human action, and when close/no-op is appropriate.
- **Preflight gate:** Pass. Preflight-needed Tickets are explicitly prohibited from going directly to coder Pods.
- **Implementation-ready handoff:** Pass. The implementation-ready path requires an `IntentPacket` and explicitly connects to `multi-agent-workflow` / worktree-based implementation.
- **Decision recording:** Pass. The `TicketComment` decision format is concrete and includes classification, evidence, inspected state, next workflow, blocking questions, and optional `IntentPacket` content.
- **Authority model:** Pass. The workflow bases routing on Ticket fields/events/artifacts plus explicitly inspected repository/Pod state, and rejects hidden conversation state as routing authority.
- **Docs update:** Pass. `docs/development/workflows.md` is small and accurately describes where the routing workflow fits among intake, preflight, worktree, multi-agent, and close/review flows.
- **Code scope:** Pass. No source-code change is needed for this ticket scope.

## 4. Blockers

None.

## 5. Non-blockers / follow-ups

None required for this ticket.

A future follow-up may be useful after the workflow is exercised: if repeated routing decisions stabilize, extract a pure classifier/test fixture. That is explicitly outside this ticket and should not block approval.

## 6. Validation assessed

- Read and assessed the Ticket record at `work-items/open/20260605-040104-ticket-orchestrator-routing/item.md`.
- Read and assessed the new workflow at `.yoi/workflow/ticket-orchestrator-routing.md`.
- Read and assessed the docs update at `docs/development/workflows.md`.
- Cross-checked related workflow boundaries against:
  - `.yoi/workflow/ticket-intake-workflow.md`
  - `.yoi/workflow/ticket-preflight-workflow.md`
  - `.yoi/workflow/multi-agent-workflow.md`
- Checked repository status/diff shape sufficiently to confirm this review concerns workflow/docs/ticket material and does not require code validation.
- Did not run `nix build .#yoi` because this is a docs/workflow-only review with no code, packaging, runtime-resource, or prompt changes requiring a package build.

## 7. Residual risk

The main residual risk is operational, not a blocker: the workflow depends on Orchestrator discipline to record decisions and inspect state explicitly until any future classifier/tooling support exists. The written workflow is clear enough for that current manual/orchestrated use.


---

<!-- event: implementation_report author: hare at: 2026-06-05T06:41:34Z -->

## Implementation report

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


---

<!-- event: close author: hare at: 2026-06-05T06:42:00Z status: closed -->

## Closed

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


---
