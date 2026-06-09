---
title: "Define commit policy for Ticket state changes from Intake and Panel"
state: "planning"
created_at: "2026-06-07T22:06:06Z"
updated_at: "2026-06-07T22:06:06Z"
---

## Background

Ticket state changes such as moving a Ticket from `intake` to `ready` modify `.yoi/tickets/.../item.md` and append thread events. During panel/Intake dogfooding, a Ticket can become `ready` from UI/action flow and leave the repository dirty without an automatic commit.

Example observed after panel interaction:

```text
M .yoi/tickets/open/20260607-213808-remove-workspace-panel-bare-letter-shortcuts/item.md
M .yoi/tickets/open/20260607-213808-remove-workspace-panel-bare-letter-shortcuts/thread.md
```

The project workflow treats Ticket records as git-authored project records, but it is currently unclear whether Intake/Panel actions should auto-commit such state transitions, prompt the user/orchestrator to commit, or intentionally leave them dirty.

## Goal

Define and implement the commit policy for Ticket state transitions performed by Intake/Panel actions, especially `planning -> ready`.

## Questions to resolve

- Should Intake/Panel auto-commit Ticket-only metadata/thread changes such as `planning -> ready`?
- If yes, which actions are safe to auto-commit?
  - `planning -> ready`
  - `ready -> queued`
  - `queued -> inprogress`
  - action_required / attention_required updates
  - comments / intake summaries
- If no, how should the UI clearly surface that a Ticket state transition left project records dirty and needs a human/orchestrator commit?
- Should commit authority differ between Companion, Intake, Orchestrator, and typed Ticket tools?
- How should this interact with worktree-based implementation branches and main workspace authority?

## Requirements

- Preserve git history as the authoritative timeline for Ticket state transitions.
- Avoid surprising broad auto-commits that include unrelated user changes.
- If auto-commit is implemented, stage only the exact Ticket paths touched by the action and use concise conventional commit messages.
- If auto-commit is not implemented, add clear UI/actionbar/diagnostic guidance when Ticket actions leave dirty Ticket files.
- Ensure Panel/Intake flows do not silently leave users thinking a transition is fully recorded when git state still needs attention.
- Do not change Ticket workflow-state semantics in this ticket unless required to express commit policy.
- Do not auto-commit implementation code changes or child worktree changes as part of Ticket action handling.

## Acceptance criteria

- The project has an explicit documented policy for commits after Intake/Panel Ticket state changes.
- `planning -> ready` behavior follows that policy consistently.
- Dirty Ticket-only state after a Panel/Intake action is either committed safely or clearly surfaced to the user.
- Tests cover the selected behavior where practical.
- `target/debug/yoi ticket doctor`, `cargo fmt --check`, and `git diff --check` pass.
