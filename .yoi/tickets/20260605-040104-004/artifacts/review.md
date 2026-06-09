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
