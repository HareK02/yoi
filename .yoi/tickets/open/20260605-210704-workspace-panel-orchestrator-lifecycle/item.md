---
id: 20260605-210704-workspace-panel-orchestrator-lifecycle
slug: workspace-panel-orchestrator-lifecycle
title: Workspace panel orchestrator lifecycle
status: open
kind: task
priority: P1
labels: [tui, pod, orchestrator, panel]
created_at: 2026-06-05T21:07:04Z
updated_at: 2026-06-05T23:32:34Z
assignee: null
legacy_ticket: null
---

## Background

The workspace orchestration panel needs a background Orchestrator Pod that is restored or spawned when the panel opens and remains alive after the panel closes.

## Requirements

- Derive Orchestrator Pod name from workspace directory, e.g. `<dir-name>-orchestrator`.
- On panel open:
  - restore if restorable;
  - attach/observe if already live;
  - spawn if missing and permitted.
- Use `.yoi/ticket.config.toml` role profile for `orchestrator`.
- Use the Ticket role launcher where practical.
- Panel close must not stop the Orchestrator.
- Surface lifecycle diagnostics in the panel.
- Do not make Orchestrator the foreground composer target by default; Companion remains foreground management chat.

## Non-goals

- Full panel UI layout.
- Intake handoff contract.
- Scheduler/lease/queue.
- Automatic coder/reviewer spawning.

## Acceptance criteria

- Workspace panel startup can ensure an Orchestrator Pod exists or report why it cannot.
- Orchestrator lifecycle uses existing Pod restore/spawn semantics and does not duplicate registry logic.
- Tests cover name derivation and restore/spawn decision logic where practical.
