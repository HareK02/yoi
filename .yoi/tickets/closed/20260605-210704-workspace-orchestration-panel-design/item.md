---
id: 20260605-210704-workspace-orchestration-panel-design
slug: workspace-orchestration-panel-design
title: Workspace orchestration panel design
status: closed
kind: task
priority: P1
labels: [tui, design, orchestration, panel]
created_at: 2026-06-05T21:07:04Z
updated_at: 2026-06-05T22:35:18Z
assignee: null
legacy_ticket: null
---

## Background

The next TUI should not be another small `:` command extension. The desired feature is a workspace-scoped orchestration panel with explicit Companion/Intake composer targets, background Orchestrator lifecycle, Intake handoff, and a user-action-oriented model.

## Requirements

Produce a design artifact that fixes:

- panel entrypoint and relation to current `--multi`;
- Companion lifecycle and identity;
- workspace Orchestrator lifecycle and naming rule (`<dir-name>-orchestrator`);
- composer target model:
  - Companion;
  - Ticket Intake;
- how Intake launch avoids writing the user request into the Companion/current Pod history;
- how Intake receives Orchestrator notification/handoff target;
- action model priorities:
  - user response required;
  - Intake draft ready;
  - Ticket ready for Go;
  - blocked/action-required;
  - review/close/preflight decisions;
  - active background work;
  - informational Pod status;
- Go semantics for a fixed Ticket;
- relationship to existing `:ticket ...` command fallback;
- failure/diagnostic behavior;
- implementation sequence.

## Non-goals

- Code implementation.
- Generic scheduler/lease/queue.
- Automatic implementation without Go/authorization.
- Replacing Ticket workflows.

## Acceptance criteria

- A design artifact exists under this Ticket's `artifacts/` directory.
- The artifact defines the state/action model and responsibility boundaries clearly enough to implement child tickets.
- The artifact defines a thin UI intermediate representation between runtime/domain state and rendering, without turning it into a second Ticket backend or scheduler.
- The artifact states that the panel is local-file-first for display, using `.yoi/tickets/` as the Ticket authority and Pod/client sockets only for live operations such as spawn/restore/send/attach.
- The artifact specifies that the concrete visual design follows existing TUI conventions/components rather than inventing a separate dashboard style.
- The artifact explicitly preserves history/context rules: dynamic messages are committed to the destination Pod history, not injected into hidden context.
- Reviewer or parent approves the design before implementation child tickets proceed.
