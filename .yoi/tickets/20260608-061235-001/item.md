---
title: "Prevent idle starvation in Ticket orchestration planning"
state: 'ready'
created_at: "2026-06-08T06:12:35Z"
updated_at: '2026-06-09T11:35:29Z'
---

## Background

The current Panel Queue automation mostly handles the transition-time event:

```text
ready -> queued
  -> notify workspace Orchestrator
```

That is not enough for robust orchestration. Queued Tickets can remain after missed notifications, Orchestrator restarts, planning returns, capacity limits, or multi-ticket coordination. The Orchestrator also needs a lightweight way to remember planned queued work across turns without relying only on session memory.

There is an existing related Ticket:

- `ticket-orchestration-plan-tool`

That Ticket asks for a TaskStore-like surface for Ticket ordering/dependency/conflict/capacity/accepted-plan records. This Ticket folds that need together with queued-backlog re-kick semantics into a narrower operational requirement:

> If runnable queued work exists and the Orchestrator is otherwise idle, the system should not wait indefinitely for another user instruction. The Orchestrator should be kicked with a bounded work set so it can either incorporate new queued work into the plan or start the next planned queued Ticket.

This is starvation prevention and explicit work-set planning, not a constant background scheduler loop.

## Goal

Implement an Orchestrator attention/re-kick policy and planning store for active Ticket work: distinguish new queued work from planned queued work and accepted in-progress work, persist the plan, and kick the Orchestrator only when work can progress and no active Orchestrator-managed operation is already being waited on.

## Planning model

The OrchestrationPlan store should distinguish at least:

- `new_queued`: Tickets with `workflow_state = queued` that have not yet been incorporated into the OrchestrationPlan.
- `planned_queued`: queued Tickets that the Orchestrator has considered and placed into an explicit plan/order/waiting set, but has not yet accepted as `inprogress`.
- `inprogress`: Tickets accepted by the Orchestrator and currently awaiting worktree/coder/reviewer/planning-sync/merge/cleanup progress.

The names do not need to become final public API names, but the state distinction is required.

## Requirements

### Active work set discovery / re-kick

- Provide a mechanism to identify Tickets that need Orchestrator attention, including at least:
  - `workflow_state = queued` Tickets not yet present in the OrchestrationPlan (`new_queued`);
  - planned queued Tickets that are not blocked/capacity-limited and can be started when there is no active in-progress work;
  - `workflow_state = inprogress` Tickets accepted by Orchestrator whose next action is not merely waiting for an active coder/reviewer/planning-sync/merge step;
  - queued Tickets left behind after Orchestrator restart, missed notification, or previous capacity stop.
- On Panel open/Orchestrator restore/spawn, or explicit user action, surface a bounded work list to the Orchestrator when there is actionable work.
- Avoid unbounded background polling. Prefer explicit events, Panel lifecycle kick, and explicit user/Orchestrator actions.
- Prevent duplicate starts: re-kick should prompt inspection/planning or acceptance of the next planned item, not blindly start coder Pods.

### Re-kick / starvation-prevention semantics

- If `new_queued` work exists and the Orchestrator is idle/not occupied by an active in-progress operation, kick or notify the Orchestrator so it can incorporate those Tickets into the plan.
- If no active `inprogress` work exists and runnable `planned_queued` work exists, kick or notify the Orchestrator so it can accept/start the next planned Ticket rather than waiting indefinitely for user instruction.
- If active `inprogress` work exists and the next expected event is coder/reviewer/planning-sync/merge completion, do not re-kick merely because queued/planned queued work also exists.
- If planned queued work is blocked, dependency-waiting, conflict-waiting, or capacity-limited, record the reason so the Panel/user can see why nothing starts.
- A re-kick is an attention signal plus bounded context, not authority to bypass `queued -> inprogress` acceptance or spawn implementation Pods without inspection.

### Orchestration plan record

- Provide or define a TaskStore-like but Ticket-domain planning surface for Orchestrator use.
- The plan should be scoped to Ticket orchestration and support records such as:
  - current active target set;
  - state bucket: `new_queued` / `planned_queued` / `inprogress` or equivalent;
  - ordering: Ticket A before Ticket B;
  - dependency/blocker: A blocks B / B blocked by A;
  - conflict: do not run A and B in parallel;
  - capacity/waiting notes;
  - accepted work plan: worktree/branch/coder/reviewer plan;
  - current next action for each target.
- Distinguish durable project-relevant routing decisions from local runtime/session claims.
  - Project-relevant decisions should live in Ticket records/thread/artifacts or a typed Ticket orchestration record under project authority.
  - Local Pod/session claims remain in the local role session registry.
- Records should survive compaction and be queryable by Ticket id/slug and relation kind.
- Keep the first version lightweight; do not implement a full scheduler/graph solver.

### Plan update semantics

- The Orchestrator should update the plan at meaningful routing boundaries:
  - new queued work incorporated into the plan;
  - queued -> inprogress acceptance;
  - inprogress -> blocked/waiting/planning/done;
  - capacity stop -> leave planned queued/waiting with reason;
  - merge-ready/done -> mark complete and consider the next planned queued Ticket if no active work remains.
- Each update should produce a bounded, inspectable record of:
  - what was considered;
  - what was incorporated into the plan;
  - what was accepted/started;
  - what was blocked/deferred/returned to planning;
  - what remains planned queued/waiting.
- Re-kick should use the current plan/work set so the Orchestrator does not forget leftover queued Tickets between turns.

### Relationship to existing work

- This Ticket should either subsume or update `ticket-orchestration-plan-tool` so there is one coherent plan/re-kick design.
- It should coordinate with:
  - `replace-intake-state-with-planning` as a prerequisite that defines the planning lane before this plan/re-kick layer builds on it;
  - `panel-close-done-tickets` for done -> closed handling;
  - local role session registry for active Pod/session ownership;
  - direct/delegation authority work for actual child Pod spawning.

## Non-requirements

- Do not turn the Panel itself into the scheduler.
- Do not auto-start unqueued Tickets.
- Do not re-kick continuously while active coder/reviewer/planning-sync/merge work is already in progress.
- Do not blindly spawn coder Pods from re-kick without Orchestrator inspection and `queued -> inprogress` acceptance.
- Do not implement a full dependency graph solver in the first version.

## Acceptance criteria

- The system can distinguish new queued work, planned queued work, and accepted in-progress work.
- New queued Tickets are not left unnoticed while the Orchestrator is otherwise idle.
- Runnable planned queued Tickets are not left unstarted when there is no active in-progress work and capacity/policy allows progress.
- The system does not re-kick merely because queued/planned work exists while Orchestrator-managed in-progress work is waiting on coder/reviewer/planning-sync/merge completion.
- Missed/stale queued Tickets can be surfaced to the Orchestrator without requiring the user to manually requeue each one.
- The Orchestrator can record and query a lightweight Ticket orchestration plan covering active targets, order/dependency/conflict/capacity, state bucket, and next actions.
- Plan records survive compaction and do not rely solely on session-lifetime TaskStore state.
- Re-kick/plan updates leave an auditable record of what was incorporated, started, blocked, returned to planning, or left waiting.
- Duplicate implementation starts are prevented by consulting current Ticket state, local role/session claims, and plan records.
- Relevant workflows/prompts/docs are updated.
- Focused tests, `target/debug/yoi ticket doctor`, `cargo fmt --check`, and `git diff --check` pass.
