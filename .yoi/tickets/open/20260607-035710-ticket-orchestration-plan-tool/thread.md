<!-- event: create author: LocalTicketBackend at: 2026-06-07T03:57:10Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: intake at: 2026-06-08T06:09:11Z -->

## Decision

## Intake refinement: readiness and boundaries

Readiness classification: `implementation_ready`, with design preflight recommended before implementation because this introduces a new Ticket/orchestration-domain tool/API surface and durable record format.

Binding decisions / invariants:
- This is a Ticket/orchestration-domain capability, not a generic `TaskStore` replacement and not a scheduler.
- Durable project-relevant ordering/dependency/conflict/capacity decisions must survive compaction and be queryable later by Ticket id/slug and relation kind.
- Local runtime claims, Pod/session assignment details, sockets, and other ephemeral execution state must remain in the local role-session/runtime registry rather than git-tracked Ticket metadata.
- The first version must remain a lightweight note/plan surface; it must not silently introduce automatic topological scheduling or unattended queue reordering.
- Orchestrator prompts/workflows should consult these records before accepting queued Tickets, but human Queue and `queued -> inprogress` acceptance semantics remain unchanged.

Implementation latitude:
- The implementer may choose the concrete storage shape within the Ticket backend, such as typed thread events, artifacts, or a small typed record file, if it satisfies queryability, validation, and durability requirements.
- The tool naming and exact relation schema can be adjusted during preflight/implementation, provided the supported relation classes remain ordering, dependency/blocker, conflict, capacity/waiting, and accepted-plan summary.
- The first implementation can favor explicit CRUD/query operations over graph solving or automatic route selection.

Escalation conditions:
- Escalate before implementing if the design would change Ticket workflow-state semantics, mutate local role-session claims, require a migration of existing Ticket records, grant mutating capabilities to Companion, or make Orchestrator start work without the explicit Queue gate.
- Escalate if relation records need to include secret/private workspace details or raw session/model output; Ticket records should store bounded project-relevant decisions only.

Validation expectations:
- Add focused tests for creating/querying relation records by Ticket and relation kind.
- Add tests or prompt/workflow coverage showing Orchestrator can consult the records before queued-ticket acceptance.
- Run the normal package validation for code/runtime-resource changes, including `nix build .#yoi` before completion.

---

<!-- event: intake_summary author: intake at: 2026-06-08T06:09:18Z -->

## Intake summary

Existing Ticket `ticket-orchestration-plan-tool` was refined for readiness. It is implementation-ready as a bounded Ticket/orchestration-domain tool surface for recording/querying ordering, dependency/blocker, conflict, capacity/waiting, and accepted-plan decisions. Needs preflight: true, because it introduces a new durable Ticket-domain API/storage surface and touches Orchestrator routing workflow semantics. Risk flags: ticket-domain-tool, persistence, workflow-routing, feature-boundary, compaction. Key invariants: do not replace TaskStore, do not implement a scheduler/graph solver, keep local Pod/session claims out of git-tracked Ticket metadata, preserve the human Queue gate and queued->inprogress acceptance contract.

---

<!-- event: state_changed author: intake at: 2026-06-08T06:09:18Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake has clarified readiness, boundaries, implementation latitude, escalation conditions, and validation expectations. The Ticket is ready for human queueing and Orchestrator routing/preflight; implementation should not start until the ready Ticket is queued and accepted through the Orchestrator workflow.

---

<!-- event: decision author: hare at: 2026-06-08T06:27:17Z -->

## Decision

## Dependency ordering: planning lane before OrchestrationPlan tool

`replace-intake-state-with-planning` owns the workflow cleanup for cases where implementation cannot be planned yet: such Tickets should return to the planning lane with a visible reason instead of remaining in a separate `preflight_needed` state/bucket.

This Ticket should assume that planning-lane behavior exists before the OrchestrationPlan tool is implemented. The dependency/order should be: settle `replace-intake-state-with-planning` first, then implement the OrchestrationPlan tool on top of the clarified planning/queued/inprogress workflow.

Do not add a Plan-tool-specific `preflight_needed` state or requirement here. At most, the tool may observe the resulting planning/queued/inprogress Ticket states produced by the workflow model.

---
