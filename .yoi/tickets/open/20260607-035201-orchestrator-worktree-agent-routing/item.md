---
id: 20260607-035201-orchestrator-worktree-agent-routing
slug: orchestrator-worktree-agent-routing
title: Orchestrator worktree agent routing
status: open
kind: task
priority: P1
labels: [orchestrator, worktree, pod, review, workflow]
workflow_state: ready
created_at: 2026-06-07T03:52:01Z
updated_at: 2026-06-07T05:49:40Z
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
- Orchestrator must record durable progress without misrepresenting branch-local review as main-branch Ticket approval:
  - main Ticket thread may record accepted worktree/branch plan, coder delegated, coder completed/blocked, reviewer delegated, and fix-loop progress;
  - reviewer verdicts for an unmerged implementation branch should be captured in the merge-ready dossier or branch-scoped review report, not as a final main-branch Ticket approval event before merge;
  - if a reviewer requests changes, the Orchestrator may record a concise progress/blocker summary in the Ticket thread, but the branch remains unapproved until the fix loop and final merge-completion phase.
- Orchestrator must not merge in this ticket's scope. It should produce a merge-ready dossier for `orchestrator-merge-completion` that includes the independent reviewer verdict and validation evidence.
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
- Progress is recorded durably, while branch-local review verdicts are kept in the merge-ready dossier/review report rather than committed as final main-branch Ticket approval before merge.
- A merge-ready dossier format is produced after branch review and validation.
- Tests or prompt/resource tests cover the worktree/coder/reviewer routing instructions and scope boundaries.
