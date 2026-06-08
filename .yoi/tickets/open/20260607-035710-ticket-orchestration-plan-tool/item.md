---
id: 20260607-035710-ticket-orchestration-plan-tool
slug: ticket-orchestration-plan-tool
title: Ticket orchestration plan tool
status: open
kind: task
priority: P1
labels: [ticket, orchestrator, planning, workflow, tools]
workflow_state: 'queued'
created_at: 2026-06-07T03:57:10Z
updated_at: '2026-06-08T11:21:42Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T11:21:13Z'
---

## Background

Orchestrator queue automation needs a lightweight way to remember routing decisions across turns while coordinating multiple Tickets. The existing Task tool is session-lifetime and human/user-visible, but Ticket ordering, dependency, conflict, and routing decisions belong to the Ticket orchestration domain.

The immediate need is not a full scheduler or dependency graph. The Orchestrator needs a small typed surface to record decisions such as:

- Ticket A should run before Ticket B;
- Ticket B is blocked by Ticket A;
- Ticket C conflicts with Ticket D and should not run in parallel;
- Ticket E is ready but waiting for capacity;
- Ticket F has been accepted by Orchestrator and has a planned worktree/branch.

These records should help the Orchestrator avoid relying only on prompt memory, especially across compaction and multi-turn routing.

## Goal

Design and implement a lightweight Ticket orchestration note/plan tool surface for Orchestrator use, similar in convenience to Task tools but scoped to Ticket ordering and routing decisions.

## Requirements

- Provide a typed, lightweight tool/API for recording and querying Orchestrator routing decisions about Tickets.
- Support at least:
  - ordering relation: before/after;
  - dependency/blocker relation: blocked_by / blocks;
  - conflict relation: do_not_parallelize / conflicts_with;
  - capacity/waiting note;
  - accepted work plan summary such as branch/worktree/pod plan when applicable.
- Distinguish durable project-relevant relations from local ephemeral execution notes:
  - dependency/order/conflict decisions that should affect future orchestration should be recorded in git-tracked Ticket records or typed Ticket thread events/artifacts;
  - local runtime claims/session details should remain in the local role session registry, not Ticket metadata.
- Keep this a Ticket/orchestration domain capability, not a generic TaskStore replacement.
- Ensure records are queryable by Ticket id/slug and by relation kind.
- Keep the first version simple; do not implement a full scheduler, graph solver, or automatic topological planner.
- Integrate with Orchestrator prompts/workflows so the Orchestrator consults these records before accepting queued Tickets.
- Avoid using session-lifetime Task tools for durable Ticket dependency/order decisions.
- Ensure compaction does not erase the authoritative record; the tool should write to a durable record path.

## Non-goals

- Replacing Ticket workflow_state.
- Replacing the local role session registry for Pod/session claims.
- Implementing full dependency graph scheduling.
- Automatically reordering all queued Tickets without Orchestrator judgment.
- Making Companion a mutating orchestration actor.

## Acceptance criteria

- Orchestrator has a simple typed way to record Ticket ordering/dependency/conflict/capacity decisions.
- The records survive compaction and are queryable in later turns.
- Project-relevant decisions are stored in the Ticket system rather than only session-lifetime Task state.
- Local runtime execution details remain out of git-tracked Ticket metadata.
- Orchestrator queue routing can consult the recorded decisions before accepting a queued Ticket.
