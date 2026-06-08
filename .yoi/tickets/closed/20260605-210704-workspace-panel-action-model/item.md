---
id: 20260605-210704-workspace-panel-action-model
slug: workspace-panel-action-model
title: Workspace panel action model
status: closed
kind: task
priority: P1
labels: [tui, ticket, orchestration, panel]
created_at: 2026-06-05T21:07:04Z
updated_at: 2026-06-05T23:31:28Z
assignee: null
---

## Background

The workspace panel should not be a plain Pod idle/working list. It should prioritize the items where the user needs to decide, respond, approve, or inspect evidence.

## Requirements

- Define and implement a workspace action model that can rank/display:
  - Intake needs user reply;
  - Intake draft ready;
  - Ticket ready for Go;
  - requirements sync needed;
  - preflight needed;
  - spike needed/running;
  - implementation running;
  - review needed;
  - blocked/action-required;
  - close-ready;
  - background informational Pod status.
- Prefer Ticket/routing/intake state over raw Pod idle state.
- Provide Go/Defer/Edit-style actions where supported by preceding tickets.
- Preserve explicit human authorization boundaries.
- Do not treat Pod completion notifications as authority; verify Ticket/Pod/output state.

## Non-goals

- Scheduler/lease/queue automation.
- Full implementation of all role actions if earlier handoff/lifecycle tickets are not landed.
- Generic issue tracker UI.

## Acceptance criteria

- Panel displays user-action-required items above passive background Pods.
- Ticket-ready Go action is easy to trigger but does not bypass Orchestrator routing/preflight gates.
- Action model is testable as plain data independent of terminal rendering.
- Existing multi-Pod status data can still be shown as lower-priority background information.
