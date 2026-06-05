---
id: 20260605-210703-workspace-orchestration-panel
slug: workspace-orchestration-panel
title: Workspace orchestration panel
status: open
kind: task
priority: P1
labels: [tui, ticket, orchestration, panel]
created_at: 2026-06-05T21:07:03Z
updated_at: 2026-06-05T23:05:06Z
assignee: null
legacy_ticket: null
---

## Background

The current TUI Ticket role commands are a command-driven MVP. They prove that TUI can launch fixed Ticket-role Pods through the client launcher, but they do not provide the desired workspace orchestration experience.

The desired panel is a workspace-scoped orchestration UI, closer to an improved `--multi` surface, but organized around Ticket/Intake/Orchestrator action state rather than raw Pod idle/working state.

## Goal

Build a workspace orchestration panel where the user can:

- chat with a Companion management interface;
- send new requests directly to Ticket Intake without putting them into the Companion/current Pod session;
- have a workspace Orchestrator restored/spawned in the background when the panel opens;
- see user-action-required Intake/Ticket states before generic Pod status;
- give a simple Go signal once an Intake-prepared Ticket is fixed and understood by the human.

## Target model

- Panel is workspace-scoped.
- Companion is the foreground management chat target.
- Orchestrator is a background coordinator Pod named from the workspace directory, e.g. `<dir-name>-orchestrator`.
- Intake is spawned per user request from the composer target.
- Intake receives Orchestrator handoff/notification target at launch.
- Panel close does not stop Orchestrator.
- The primary list is an action model, not a Pod status list.

## Child tickets

1. `workspace-orchestration-panel-design`
   - Produce the detailed architecture/design artifact for the panel.

2. `workspace-panel-orchestrator-lifecycle`
   - Restore/spawn workspace Orchestrator when the panel opens; leave it running on panel close.

3. `workspace-panel-composer-targets`
   - Add composer targets for Companion vs Ticket Intake; Intake sends the composer body directly to a new Intake Pod.

4. `ticket-intake-orchestrator-handoff`
   - Define and implement Intake -> Orchestrator handoff/notification contract.

5. `workspace-panel-action-model`
   - Display user-action-required Intake/Ticket states ahead of raw Pod status and support Go/Defer/Edit-style decisions.

## Non-goals

- Generic scheduler/lease/queue automation.
- Arbitrary role registry.
- Replacing the single-Pod TUI.
- Stopping background Orchestrator on panel close.
- Bypassing Ticket Intake / Routing / Preflight / Review gates.

## Acceptance criteria

- Child tickets are created and sequenced.
- The design ticket fixes the panel responsibility boundary before implementation tickets proceed.
- Implementation child tickets use existing Ticket tools/config/launcher where possible.
- The command-driven `:ticket ...` path remains available as a low-level fallback, but the panel design does not depend on users typing commands for the main flow.
