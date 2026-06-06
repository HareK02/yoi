---
id: 20260605-210704-workspace-panel-composer-targets
slug: workspace-panel-composer-targets
title: Workspace panel composer targets
status: open
kind: task
priority: P1
labels: [tui, composer, intake, panel]
created_at: 2026-06-05T21:07:04Z
updated_at: 2026-06-06T04:24:53Z
assignee: null
legacy_ticket: null
---

## Background

The workspace panel composer must let users choose whether a message goes to the Companion management chat or directly to Ticket Intake.

The key UX requirement is that a request sent to Intake must not be appended to the Companion/current Pod session history.

## Requirements

- Add a composer target model for the workspace panel:
  - Companion;
  - Ticket Intake.
- Companion target sends normal messages to the Companion Pod/session.
- Intake target launches a new Intake role Pod and sends the composer body as its first `Method::Run` input.
- Intake target must not send the body to Companion/current Pod history.
- Empty Intake messages are rejected.
- The UI clearly shows the active composer target.
- User can switch/cancel target without losing typed text where practical.
- Use Ticket role launcher rather than constructing spawn/profile/workflow prompt content inside UI.

## Non-goals

- Generic arbitrary target routing.
- Scheduler/queue.
- Intake -> Orchestrator handoff implementation.
- Action queue display.

## Acceptance criteria

- User can choose Companion or Intake before pressing Enter.
- Intake path creates a new Intake role launch with the typed body as dynamic run input.
- Existing Companion history does not receive the Intake body.
- Tests cover target switching and Enter behavior for both targets where practical.
