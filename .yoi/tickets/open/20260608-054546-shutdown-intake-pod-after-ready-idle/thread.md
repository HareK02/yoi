<!-- event: create author: LocalTicketBackend at: 2026-06-08T05:45:46Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-08T06:18:12Z -->

## Intake summary

Implementation-ready: implement post-idle self-shutdown for Ticket Intake role Pods after a successful `TicketIntakeReady`, preserving tool result/final response/history commits and leaving local Ticket claim/session records intact. needs_preflight=true due to Pod lifecycle, session/history ordering, role/session detection, and panel/local-claim behavior. risk_flags=[pod-lifecycle, session-history, ticket-workflow, panel-claim].

---

<!-- event: state_changed author: ticket-intake at: 2026-06-08T06:18:12Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake complete; Ticket has explicit goal, requirements, ordering constraints, invariants, acceptance criteria, validation expectations, and escalation risks for Orchestrator routing.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T06:20:43Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T06:21:07Z -->

## Decision

Routing decision: preflight_needed

Reason:
- The Ticket is well scoped, but Intake explicitly identified `needs_preflight=true` due to Pod lifecycle, session/history ordering, role/session detection, and panel/local-claim behavior.
- The required shutdown must happen after tool result delivery, assistant final response, history/session commits, and the Idle transition. That ordering crosses tool-result handling, Worker/Pod lifecycle state, controller shutdown, and role-session metadata boundaries.
- The design must also fix which component owns the transient `shutdown_after_idle` signal and how Intake-role detection is performed without injecting hidden model-visible instructions.

Evidence checked:
- Ticket body: background, requirements, design considerations, acceptance criteria.
- Thread: Intake summary and latest `ready -> queued` event.
- Workspace state: no matching branch/worktree exists; active `split-direct-and-delegation-authority` worktree is separate.
- Visible Pods: many completed Intake peer Pods remain idle/live, which is the behavior this Ticket aims to address; no implementation Pod exists for this Ticket.

Next action:
- Run `ticket-preflight-workflow` before implementation delegation.
- Preflight should record: exact lifecycle hook/owner for the shutdown-after-idle flag, success signal source for `TicketIntakeReady`, role/session detection source, ordering relative to tool result/history commit/assistant response/Idle, behavior if another user turn arrives before shutdown, failure diagnostics, claim preservation expectations, and focused test strategy.
- Leave this Ticket queued for now; do not transition `queued -> inprogress`, create `.worktree/shutdown-intake-pod-after-ready-idle`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- Shutdown-after-idle cannot be represented as transient runtime state without hidden context injection.
- Role/session detection for Intake Pods is not durable enough to distinguish Companion/Orchestrator/Coder/Reviewer callers.
- Clean shutdown after Idle conflicts with current controller/session commit ordering.
- Preserving local claims while stopping the Pod requires changes to claim semantics rather than lifecycle state only.

---
