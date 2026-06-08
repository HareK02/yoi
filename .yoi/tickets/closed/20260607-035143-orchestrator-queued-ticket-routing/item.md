---
id: 20260607-035143-orchestrator-queued-ticket-routing
slug: orchestrator-queued-ticket-routing
title: Orchestrator queued Ticket routing
status: closed
kind: task
priority: P1
labels: [panel, orchestrator, ticket, routing, workflow]
workflow_state: done
created_at: 2026-06-07T03:51:43Z
updated_at: 2026-06-07T05:13:36Z
assignee: null
---

## Background

`workspace-panel-orchestrator-queue-automation` defines the overall goal: Panel Queue should notify the workspace Orchestrator, and the Orchestrator should route queued Tickets and start work when unblocked. The first implementation slice should focus on routing acceptance, not full worktree/coder/reviewer/merge execution.

This ticket uses the current workflow contract as the basis: Queue is a human gate, `queued` means Orchestrator may route, and `inprogress` is the durable Orchestrator acceptance marker before implementation side effects.

## Goal

Implement the queued Ticket routing entrypoint for the workspace Orchestrator.

## Requirements

- Fire a durable Orchestrator notification when a Panel action transitions a Ticket `ready -> queued`.
- Notification content must instruct the Orchestrator to:
  - read the queued Ticket;
  - inspect current workspace state;
  - start if unblocked;
  - record a blocker/defer reason if not startable.
- Update wording away from passive "implementation was not started" semantics. Queue authorizes routing; Orchestrator decides whether to start.
- Orchestrator routing workflow/prompt must define the acceptance order:
  - inspect Ticket/workspace state;
  - if unblocked, transition `queued -> inprogress`;
  - do not create worktrees or spawn implementation/review Pods before `inprogress` acceptance;
  - if `queued -> inprogress` fails, do not create side effects.
- First-pass blocker detection should be intentionally small and explicit:
  - Ticket is no longer `queued`;
  - `attention_required` is set;
  - explicit dependency/blocker/preflight gap is recorded in the Ticket;
  - same Ticket already has an active local role/session/worktree;
  - branch/worktree name collision;
  - main workspace is clearly unsafe for orchestration;
  - a current in-progress Ticket clearly conflicts based on explicit Ticket text or known scope.
- If blocked, Orchestrator records a concise decision/comment and leaves the Ticket queued or explicitly defers through existing Ticket status/state mechanisms.
- Progress/acceptance should be recorded in the Ticket thread using existing typed events/comments where available.
- Use Ticket lifecycle domain tools once available; do not model this as an Orchestrator-specific Ticket tool feature.

## Non-goals

- Implementing coder/reviewer worktree routing; that belongs to `orchestrator-worktree-agent-routing`.
- Implementing merge/close completion; that belongs to `orchestrator-merge-completion`.
- Implementing dynamic workflow state-machine tool gating.
- Implementing composite `StartTicketWork` unless it is trivial and does not replace the prompt/workflow contract.
- Polling newly-created `intake` Tickets or auto-starting Intake.

## Acceptance criteria

- Panel Queue produces a durable Orchestrator routing notification with correct semantics.
- Orchestrator prompt/workflow states that `queued -> inprogress` acceptance must precede worktree/SpawnPod side effects.
- A queued Ticket can be accepted and marked `inprogress` by Orchestrator routing when unblocked.
- Blocked queued Tickets get a recorded reason rather than silent no-op.
- Tests or prompt/resource tests cover Queue notification wording and the acceptance-order instruction.
