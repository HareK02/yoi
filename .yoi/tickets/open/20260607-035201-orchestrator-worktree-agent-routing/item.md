---
id: 20260607-035201-orchestrator-worktree-agent-routing
slug: orchestrator-worktree-agent-routing
title: Orchestrator worktree agent routing
status: open
kind: task
priority: P1
labels: [orchestrator, worktree, pod, review, workflow]
workflow_state: intake
created_at: 2026-06-07T03:52:01Z
updated_at: 2026-06-07T03:52:01Z
assignee: null
legacy_ticket: null
---

## Background

After a queued Ticket is accepted as `inprogress`, the Orchestrator should be able to route implementation work through the existing `worktree-workflow` and `multi-agent-workflow` operational model. The current workflows already describe the desired behavior: create a child worktree, spawn a coder Pod with scoped write access, spawn an independent reviewer sibling, and run review/fix loops.

This ticket is the worktree/coder/reviewer execution slice under `workspace-panel-orchestrator-queue-automation`.

## Goal

Make the workspace Orchestrator execute accepted in-progress Tickets through worktree + coder/reviewer Pod routing using the existing workflow contracts as builtin/role guidance.

## Requirements

- Use `worktree-workflow` as the mechanical worktree creation/management contract.
- Use `multi-agent-workflow` as the coder/reviewer sibling orchestration contract.
- Orchestrator must only start this path after the Ticket is accepted as `inprogress`.
- Create one implementation worktree per Ticket/bounded task under `.worktree/<task-name>`.
- Exclude `.yoi` from child worktrees and keep Ticket/thread/workflow updates in the main workspace.
- Spawn coder Pods with read access to the main workspace and write access only to the child worktree or narrower implementation scope.
- Spawn reviewer Pods as Orchestrator siblings, not coder children, with read-only scope by default.
- Coder task prompts must include an intent packet: intent, requirements, invariants, non-goals, escalation conditions, validation expectations, worktree path, and branch.
- Reviewer task prompts must focus on Ticket intent, diff, invariants, validation adequacy, and blocker/non-blocker classification.
- Orchestrator must record progress in the Ticket thread at meaningful checkpoints:
  - accepted worktree/branch plan;
  - coder delegated;
  - coder completed or blocked;
  - reviewer delegated;
  - review approved/requested changes;
  - fix loop outcome.
- Orchestrator must not merge in this ticket's scope. It should produce a merge-ready dossier for `orchestrator-merge-completion`.
- If compaction occurs, workflow obligations should remain understandable; relate to `preserve-active-workflows-across-compaction` but do not block first implementation on it.

## Non-goals

- Queue notification / queued acceptance; that belongs to `orchestrator-queued-ticket-routing`.
- Merge/cleanup/close; that belongs to `orchestrator-merge-completion`.
- Broad conflict/dependency inference beyond the first-pass routing decision.
- Dynamic workflow-state tool gating.
- Letting coder Pods edit main workspace Ticket records or `.yoi`.

## Acceptance criteria

- Orchestrator has builtin/role guidance or implementation wiring that follows `worktree-workflow` and `multi-agent-workflow` for accepted Tickets.
- Worktree creation excludes `.yoi`.
- Coder and reviewer Pods are spawned as siblings under Orchestrator with the expected scopes.
- Progress is recorded in the Ticket thread.
- A merge-ready dossier format is produced after review approval and validation.
- Tests or prompt/resource tests cover the worktree/coder/reviewer routing instructions and scope boundaries.
