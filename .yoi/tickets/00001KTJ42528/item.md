---
title: "Reduce Ticket lifecycle commit noise"
state: "planning"
created_at: "2026-06-07T22:43:09Z"
updated_at: "2026-06-07T22:43:09Z"
---

## Background

Recent Ticket-driven / multi-agent work has produced many small commits where most commits are Ticket record lifecycle events rather than implementation changes. A typical implemented Ticket currently creates commits such as:

```text
ticket: planning ...
ticket: delegate ...
feat/fix: ...
ticket: report implementation ...
ticket: approve ...
merge: ...
ticket: close ...
```

This keeps a detailed audit trail, but it makes project history look noisy and overly fine-grained. Recent examples also include many one-off follow-up Ticket creation commits and automated Panel/Orchestrator record commits.

The desired direction is to keep the Ticket audit trail, but reduce commit count by batching compatible Ticket record updates into fewer, more meaningful commits.

## Goal

Define and implement a lower-noise commit policy for Ticket lifecycle records produced by manual orchestration, Panel actions, Intake, and multi-agent workflows.

## Requirements

- Preserve git history as the authoritative timeline for Ticket records.
- Reduce commit count caused by one-event-per-commit Ticket record updates.
- Define which Ticket lifecycle events should be batched together.
- Candidate batching policy:
  - Combine planning + delegation into one `ticket: delegate ...` commit when done in one orchestration step.
  - Combine implementation report + reviewer approval into one `ticket: approve ...` commit when approval is known before committing the record.
  - Batch multiple newly discovered follow-up Tickets into one `ticket: add ... tasks` commit when created in the same conversation/decision burst.
  - Keep close commits separate when they move open -> closed and write `resolution.md`, unless a better policy is explicitly chosen.
- Decide how automated Panel/Intake/Orchestrator Ticket updates should be committed or surfaced so they do not create excessive micro-commits.
- Update relevant workflow docs/prompts so agents stop committing every single Ticket event mechanically.
- Avoid losing important review/approval evidence; batching should change commit grouping, not remove Ticket thread events.
- Avoid broad history rewriting as part of implementation unless explicitly requested.

## Acceptance criteria

- Workflows document the preferred lower-noise commit grouping for Ticket lifecycle events.
- Agents following `/multi-agent-workflow`, `/worktree-workflow`, and Ticket role flows have clear guidance on when to batch Ticket records.
- Ticket close / merge / approval evidence remains auditable in files.
- Automated Panel/Intake/Orchestrator Ticket updates have a documented commit/dirty-state policy aligned with `define-ticket-action-commit-policy` or reference that ticket if it remains separate.
- No implementation code changes are mixed with this policy update unless required for automation support.
- `target/debug/yoi ticket doctor`, `cargo fmt --check`, and `git diff --check` pass.
