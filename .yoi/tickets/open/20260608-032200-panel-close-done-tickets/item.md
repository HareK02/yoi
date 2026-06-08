---
id: '20260608-032200-panel-close-done-tickets'
slug: 'panel-close-done-tickets'
title: 'Close done Tickets from workspace panel'
status: 'open'
kind: 'task'
priority: 'P2'
labels: ['tui', 'panel', 'ticket', 'close', 'workflow-state']
workflow_state: 'inprogress'
created_at: '2026-06-08T03:22:00Z'
updated_at: '2026-06-08T05:50:11Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T05:46:20Z'
---

## Background

Panel currently derives `NextUserAction::Close` for Tickets whose `workflow_state` is `done`, but the action is a safe no-op:

```text
Close for Ticket <slug> requires explicit resolution text; no close was recorded.
```

This leaves Tickets in an awkward state:

```text
status: open
workflow_state: done
```

The Ticket backend `close` operation is more than a raw directory move: it moves the Ticket to `closed/`, updates `item.md`, writes `resolution.md`, and records a close/resolution event. However, when a Ticket is already `workflow_state: done`, the completion judgment has already happened. Panel does not need to launch an LLM worker just to produce a resolution.

## Goal

Implement Panel `Close` action for `workflow_state: done` Tickets by calling the Ticket backend close operation with a deterministic generated resolution.

## Requirements

- Implement `NextUserAction::Close` dispatch in the TUI/Panel path.
- Only auto-close when the Ticket is safe to close, at minimum:
  - local status is `open`;
  - `workflow_state == done`;
  - `attention_required` is not set;
  - `action_required` is not set or otherwise not blocking close;
  - `resolution.md` does not already exist.
- Generate a deterministic resolution without invoking an LLM worker.
- Resolution should be concise and factual, e.g.:

```md
Closed from workspace panel.

The Ticket had already reached `workflow_state: done`.
No implementation or workflow-state changes were started by this close action.
See `thread.md` for implementation, review, merge, validation, and state-change records.
```

- Call the existing Ticket backend close path/tooling rather than manually moving files.
- After close, refresh Panel state so the Ticket disappears from open rows / appears in closed history as appropriate.
- If the Ticket is not safe to close, show a bounded diagnostic explaining the blocker instead of doing nothing.
- Do not close Tickets whose workflow state is not `done`.
- Do not launch Orchestrator/Companion/worker solely to generate resolution text.
- Do not auto-commit the close in this ticket unless the separate commit-policy work decides that behavior.

## Design notes

This is primarily a TUI/Panel feature. The Ticket backend already exposes a close operation requiring resolution text; the Panel should supply a deterministic generated resolution for already-done Tickets and invoke that backend operation.

## Acceptance criteria

- Pressing empty `Enter` on a `workflow_state: done` open Ticket in Panel closes it through the Ticket backend.
- The closed Ticket has `resolution.md` with deterministic generated text.
- The close is recorded in the Ticket thread/status according to existing backend behavior.
- Unsafe close attempts produce clear diagnostics and do not mutate the Ticket.
- Panel refreshes after close.
- Tests cover successful close, blocked close, and generated resolution content.
- Relevant TUI/Ticket tests, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
