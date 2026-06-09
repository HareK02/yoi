<!-- event: create author: LocalTicketBackend at: 2026-06-08T06:12:35Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-08T06:15:37Z -->

## Decision

## Scope refinement: avoid idle starvation, not constant iteration

The phrase "continuously iterate" should not mean that the Orchestrator constantly re-kicks itself regardless of current work. The desired behavior is narrower:

- Do not leave newly queued work unnoticed while the Orchestrator is otherwise idle.
- Do not leave planned queued work unstarted when there is no in-progress work and capacity/policy allows starting it.
- Do not re-kick just because queued work exists if the Orchestrator is already waiting for an active coder/reviewer/preflight/merge step.

Refined planning model:

- `new_queued`: Tickets with `workflow_state = queued` that have not yet been incorporated into the OrchestrationPlan.
- `planned_queued`: queued Tickets that the Orchestrator has considered and placed into an explicit plan/order/waiting set, but has not yet accepted as `inprogress`.
- `inprogress`: Tickets accepted by the Orchestrator and currently awaiting worktree/coder/reviewer/preflight/merge/cleanup progress.

Desired scheduling/re-kick semantics:

1. If there is `new_queued` work and the Orchestrator is idle/not occupied by an active in-progress operation, it should be kicked or shown a bounded work list so it can incorporate the new Tickets into the plan.
2. If there is no `inprogress` work and there is `planned_queued` work that is not blocked/capacity-limited, the Orchestrator should be kicked to start/accept the next planned Ticket rather than waiting indefinitely for user instruction.
3. If there is active `inprogress` work whose next expected event is coder/reviewer/preflight/merge completion, do not re-kick merely because queued/planned work exists. The Orchestrator should wait for the active work or an explicit user action/capacity decision.
4. If planned queued work is blocked or waiting for capacity, the plan should record that reason so the panel/user can see why nothing starts.

This should be framed as starvation prevention and explicit work-set planning, not a background scheduler loop.

---

<!-- event: decision author: hare at: 2026-06-08T06:27:33Z -->

## Decision

## Dependency ordering: planning lane before re-kick planning layer

`replace-intake-state-with-planning` owns the behavior for cases where work cannot be planned/implemented yet because requirements or preflight are missing: return the Ticket to the planning lane with a visible reason.

This Ticket assumes that behavior exists and builds the `new_queued` / `planned_queued` / `inprogress` re-kick model on top of the clarified workflow states. Implementation order should be: settle `replace-intake-state-with-planning` first, then implement the OrchestrationPlan/re-kick layer.

Therefore, the OrchestrationPlan/re-kick layer should not introduce its own separate `preflight_needed` bucket or cleanup logic.

---
