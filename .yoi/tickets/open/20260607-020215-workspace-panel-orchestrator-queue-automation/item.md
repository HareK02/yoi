---
id: 20260607-020215-workspace-panel-orchestrator-queue-automation
slug: workspace-panel-orchestrator-queue-automation
title: Workspace panel Orchestrator queue automation
status: open
kind: task
priority: P1
labels: [panel, orchestrator, ticket, automation, workflow]
workflow_state: intake
created_at: 2026-06-07T02:02:15Z
updated_at: 2026-06-07T03:57:24Z
assignee: null
legacy_ticket: null
---

## Background

The panel currently has pieces of the Ticket orchestration path:

- Panel Ticket Intake can start an Intake role Pod from a user instruction.
- Intake can materialize or update a Ticket and mark it `workflow_state = ready`.
- Panel can Queue a ready Ticket, transitioning `ready -> queued` and notifying the workspace Orchestrator.

However, the intended semantics of `queued` are stronger than a passive notification. Once a human queues a Ticket, the Orchestrator should treat it as eligible for routing and proceed if the workspace state allows it. The Orchestrator, not the panel, should decide whether work can start now or should wait because of conflicts, dependencies, capacity, or preflight gaps.

The current route is not yet a complete automated path from Panel/Intake through Orchestrator-managed implementation, review, merge, and Ticket completion.

## Goal

Implement a first-class Panel -> Intake/Queue -> Orchestrator automation path where queued Tickets are actively routed by the Orchestrator and, when unblocked, progressed through implementation/review/merge/close workflow.

## Target semantics

- `ready`: Ticket is materialized and can be queued by the human.
- `queued`: Human has authorized Orchestrator routing. The Orchestrator should inspect current state and start work if unblocked.
- `inprogress`: Orchestrator has accepted the Ticket as its durable responsibility marker and owns routing/coordination until completion, explicit defer, or explicit block. It does not merely mean that a coder Pod is already running.
- `done`: Implementation/review/merge outcome is complete and the Ticket can be closed or has been closed.

## Orchestrator acceptance contract

- `inprogress` is the durable Orchestrator acceptance marker for a queued Ticket.
- Orchestrator must inspect the Ticket and current workspace state before accepting queued work.
- Orchestrator must transition `queued -> inprogress` before creating implementation worktrees or spawning implementation/review Pods, unless a typed operation performs the acceptance and side-effect setup atomically.
- If `queued -> inprogress` fails, Orchestrator must not spawn implementation/review Pods for that Ticket.
- After `inprogress` acceptance, worktree/spawn/first-run failures should be recorded as progress/block/failure on the in-progress Ticket rather than silently reverting to queued or starting a duplicate route.
- The short-term implementation may enforce this through Orchestrator prompt/workflow text plus the existing typed Ticket workflow-state tool.
- The long-term target is a typed Orchestrator operation such as `AcceptQueuedTicket` or `StartTicketWork` that validates the queued state, records acceptance, establishes any local claim/lease, and returns the bounded context needed for worktree/Pod launch.

## Requirements

- Update the panel Queue notification/prompt text so it clearly tells the Orchestrator to route the queued Ticket and start if unblocked, rather than implying that implementation must not start.
- Add Orchestrator-side handling for queued Tickets:
  - read the Ticket and current workflow state;
  - inspect active worktrees/branches/Pods and queued/inprogress Tickets where available;
  - decide whether there are conflicts, dependency blockers, preflight gaps, or capacity constraints;
  - if unblocked, transition `queued -> inprogress` using the typed Ticket workflow tool/path before implementation side effects;
  - if blocked, record a concise reason and leave the Ticket queued or defer through the existing Ticket status/state mechanism as appropriate.
- Ensure Orchestrator cannot create implementation worktrees or spawn implementation/review Pods for a queued Ticket until the Ticket is already accepted as `inprogress` or the same typed operation atomically accepts it.
- Add or design toward a typed accept/start operation for Orchestrator routing so the `queued -> inprogress` acceptance contract is not only prompt-enforced.
- Connect Orchestrator routing to the existing Ticket workflows:
  - `ticket-preflight-workflow` when requirements/design readiness are uncertain;
  - `worktree-workflow` for worktree creation/management;
  - `multi-agent-workflow` or equivalent sibling coder/reviewer routing for implementation and review.
- Ensure Orchestrator can spawn or coordinate implementation/review Pods only after the queued Ticket has been accepted as in progress and scope/worktree boundaries are clear.
- Carry enough run/assignment state for the panel to show that a queued Ticket has been accepted or is blocked/waiting.
- Define the merge/completion boundary explicitly:
  - for local dogfooding, Orchestrator may proceed through merge/cleanup when the queued workflow and current policy authorize it;
  - otherwise it must stop at a merge-ready dossier with a clear human action required.
- After successful merge/validation, record implementation/report/review events and close or mark the Ticket done according to the typed Ticket workflow state rules.
- Preserve the human Queue gate: newly ready Tickets must not be automatically started without being queued.
- Preserve no-polling semantics for Intake creation; the automation begins from explicit Queue or explicit Orchestrator action, not background file polling.
- Coordinate with `workspace-panel-local-role-session-registry` if local role/session assignment is needed to prevent duplicate Orchestrator/coder/reviewer ownership.

## Non-goals

- Making the panel itself a scheduler.
- Starting implementation directly from Intake before the Ticket reaches `ready` and is queued.
- Ignoring conflicts with existing active worktrees, branches, or in-progress Tickets.
- Blindly spawning coder Pods from a notification without reading the Ticket and current workspace state.
- Replacing the existing Ticket workflow documents; this ticket should wire them into the Orchestrator route.
- Reintroducing direct selected-Pod send from the panel.

## Acceptance criteria

- Queue notification/prompt semantics say that Orchestrator should route and start if unblocked.
- A queued Ticket can be accepted by the Orchestrator, moved to `inprogress`, and routed to implementation through existing worktree/coder/reviewer workflows.
- Implementation/review Pod spawning is prevented for a queued Ticket unless the Ticket has already been accepted as `inprogress` or a typed accept/start operation performs that transition atomically.
- If acceptance fails, no implementation side effects occur; if post-acceptance side effects fail, the failure/block is recorded on the in-progress Ticket.
- Blocking conditions are recorded instead of silently doing nothing.
- The panel can distinguish at least queued-waiting, in-progress, and blocked/deferred outcomes from Orchestrator routing.
- The path from Panel Intake -> ready Ticket -> Queue -> Orchestrator routing is documented and covered by focused tests or integration-style unit tests around the routing logic.
- The merge/completion boundary is explicit and validated against current policy before automatic merge/close is allowed.
