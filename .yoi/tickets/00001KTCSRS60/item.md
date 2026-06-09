---
title: "Ticket intake to orchestrator handoff"
state: "closed"
created_at: "2026-06-05T21:07:04Z"
updated_at: "2026-06-06T05:27:40Z"
---

## Background

Intake can create or refine Tickets, but the next Orchestrator routing step is currently manual. The workspace panel needs a safe handoff from Intake to the workspace Orchestrator.

## Requirements

- Define a machine-readable handoff contract from Intake to Orchestrator.
- Handoff should include:
  - created/updated Ticket id or slug;
  - readiness;
  - needs_preflight;
  - risk flags when available;
  - whether user Go/approval is required;
  - summary of the Intake result.
- Intake launch should receive the Orchestrator notification target in its initial input or durable metadata, not hidden context.
- Orchestrator should be notified through an existing durable/observable channel where possible.
- Handoff should not automatically start implementation.
- The panel should be able to show a Ticket-ready-for-Go or routing-needed state after Intake.

## Non-goals

- Full scheduler/queue/lease.
- Automatic coder/reviewer spawn.
- Generic notification bus redesign.
- External tracker integration.

## Acceptance criteria

- Intake can hand off a newly created/refined Ticket to the workspace Orchestrator without the user manually retyping the Ticket id.
- Handoff is visible/auditable in history or Ticket records.
- Orchestrator routing is triggered or queued only within the agreed Go/authorization boundary.
- Tests or fixtures cover handoff payload parsing/formatting where practical.
