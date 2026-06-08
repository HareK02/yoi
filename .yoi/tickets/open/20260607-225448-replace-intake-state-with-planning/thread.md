<!-- event: create author: LocalTicketBackend at: 2026-06-07T22:54:48Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-08T07:07:59Z -->

## Decision

## Decision: remove standalone `preflight` concept; use return-to-planning

Do not preserve `preflight` as a separate workflow concept, operation, or long-lived routing bucket.

The desired model is:

- Intake/Planning prepares a Ticket until it can become `ready`.
- Orchestrator reads queued/ready work and may find that implementation cannot be planned safely yet.
- In that case, Orchestrator records the missing information/decision and returns the Ticket to `planning` with a visible reason.
- There is no separate `preflight_needed` state, no separate preflight lane, and no Intake-authored `needs_preflight` gate.

Responsibility split:

- Intake/Planning owns requirements clarification and Ticket shaping.
- Orchestrator owns routing and may reject/return work to planning when the Ticket lacks required decisions, constraints, or implementation intent.
- Planning owns resolving that returned reason.

Workflow/documentation implications:

- Remove `needs_preflight` from Intake-facing concepts. Intake may record `risk_flags`, open questions, and readiness, but should not decide that a future Orchestrator preflight operation is required.
- Replace Orchestrator `preflight_needed` classification with a planning-return classification such as `planning_needed` / `return_to_planning` / `requirements_sync_needed`.
- Remove or fold `ticket-preflight-workflow` into the planning workflow/docs. The useful content is the checklist for deciding whether a Ticket has enough binding decisions and implementation intent; it should not be exposed as a separate operation/lane.
- Risk flags are reviewer/orchestrator attention markers, not stop gates.
- If Orchestrator cannot name a concrete missing decision/information item, it should not return the Ticket to planning merely because the area is risky; it should proceed with an IntentPacket plus escalation/reviewer focus.

Acceptance impact:

- The implementation should update workflow-state terminology, prompts/workflows, panel labels/actions, and tests so missing implementation readiness is represented as a return to `planning`, not as `preflight_needed`.
- Existing mentions of `preflight_needed` should be removed, renamed, or treated as migration/legacy compatibility only where needed.

---

<!-- event: intake_summary author: ticket-intake-workflow at: 2026-06-08T07:22:47Z -->

## Intake summary

Existing Ticket was refined as an implementation-ready planning-state replacement task. The binding direction is to replace the user-facing `intake` workflow state with a planning-oriented state/lane, preserve the Intake role name as separate terminology, and remove standalone `preflight` / `needs_preflight` concepts from the user-facing workflow. Orchestrator missing-readiness routing should record a concrete reason and return the Ticket to planning, not create a separate preflight lane. Implementation should update state parsing/normalization or migration, transition graph, panel labels/actions, role/workflow prompts/docs, Ticket tools/CLI surfaces, and tests. Risk flags for routing/review attention: workflow-state, migration, panel-ux, orchestrator-routing, prompt-workflow-docs. No separate preflight gate remains; risk flags are attention markers, not blockers.

---

<!-- event: state_changed author: ticket-intake-workflow at: 2026-06-08T07:22:47Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket has enough binding decisions, invariants, acceptance criteria, and validation expectations for Orchestrator routing. Marking ready for user queueing; implementation must not start until the user/panel performs `ready -> queued` and Orchestrator accepts it.

---
