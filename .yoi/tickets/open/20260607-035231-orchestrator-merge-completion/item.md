---
id: 20260607-035231-orchestrator-merge-completion
slug: orchestrator-merge-completion
title: Orchestrator merge completion
status: open
kind: task
priority: P1
labels: [orchestrator, merge, ticket, workflow, validation]
workflow_state: ready
created_at: 2026-06-07T03:52:31Z
updated_at: 2026-06-07T06:46:06Z
assignee: null
legacy_ticket: null
---

## Background

Once Orchestrator-managed implementation and independent review have produced a merge-ready dossier, the final automation slice is merging, validating, cleanup, and Ticket completion. The current `multi-agent-workflow` already describes the desired manual/orchestrated behavior. This ticket makes that behavior explicit for workspace Orchestrator automation.

Merge authority must be explicit. In this dogfooding workspace, Orchestrator may proceed through merge/cleanup/close when workflow/policy conditions are met. More conservative/public defaults may stop at merge-ready dossier instead.

## Goal

Implement Orchestrator merge/completion behavior for reviewed in-progress Tickets, based on the existing multi-agent workflow.

## Requirements

- Define the merge authority boundary clearly:
  - in local dogfooding mode/workspace policy, Orchestrator may merge/cleanup/close after review approval and validation;
  - otherwise Orchestrator must stop at a merge-ready dossier with human action required.
- Require an independent reviewer approval in the merge-ready dossier before merge, unless a human explicitly overrides and records a decision.
- Treat the reviewer verdict before merge as branch-scoped evidence, not a final main-branch Ticket approval event.
- Commit main-branch Ticket review/approval and completion records only in the merge-completion phase, after the branch is merged or as part of the same controlled completion sequence, so the main Ticket history does not claim approval for code that is not in main.
- Require a merge-ready dossier including:
  - Ticket id/slug;
  - branch/worktree;
  - commits;
  - intent/invariant check;
  - implementation summary;
  - coder/reviewer Pods;
  - blockers fixed or rejected findings with reasons;
  - validation performed;
  - residual risks;
  - dirty state.
- Before merge, verify main workspace state is safe and unrelated dirty changes are understood.
- Merge with `git merge --no-ff <branch>` or the agreed project merge method.
- Run post-merge validation appropriate to the change. Minimum baseline should include:
  - focused tests from the ticket/dossier;
  - `cargo fmt --check`;
  - `git diff --check`;
  - `target/debug/yoi ticket doctor` where applicable.
- Use broader validation (`cargo check --workspace --all-targets`, `nix build .#yoi`, etc.) when risk or touched files warrant it.
- Record review/merge/validation outcome in the Ticket thread during the merge-completion phase, after confirming that the reviewed branch is the branch being merged.
- Transition `inprogress -> done` or close the Ticket according to typed Ticket workflow rules.
- Remove merged child worktree and delete merged branch unless the user/workflow explicitly asks to keep them.
- Stop coder/reviewer Pods and reclaim scopes after completion.

## Non-goals

- Queue notification / queued acceptance; that belongs to `orchestrator-queued-ticket-routing`.
- Worktree/coder/reviewer routing; that belongs to `orchestrator-worktree-agent-routing`.
- Removing the human override path for unusual reviews/merge decisions.
- Making merge authority implicit for all public/default configurations.
- Solving active workflow compaction persistence; relate to `preserve-active-workflows-across-compaction`.

## Acceptance criteria

- Orchestrator has explicit merge/completion guidance or implementation wiring based on `multi-agent-workflow`.
- Reviewed in-progress Tickets can be merged, validated, closed/done, and cleaned up by Orchestrator in authorized dogfooding mode.
- Conservative mode or missing authorization stops at merge-ready dossier rather than merging.
- Ticket thread records branch-reviewed merge/completion decision and validation evidence in the merge-completion phase, not as premature main-branch approval before merge.
- Worktrees/branches/Pods are cleaned up after successful merge/close.
- Tests or prompt/resource tests cover merge authority boundary and required dossier/validation fields.
