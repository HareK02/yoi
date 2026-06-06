---
id: 20260606-052903-workspace-panel-ticket-action-dispatch
slug: workspace-panel-ticket-action-dispatch
title: Workspace panel Ticket action dispatch
status: open
kind: task
priority: P1
labels: [tui, ticket, orchestration, panel]
created_at: 2026-06-06T05:29:03Z
updated_at: 2026-06-06T05:30:26Z
assignee: null
legacy_ticket: null
---

## Background

The first workspace panel implementation now has `yoi panel`, Ticket/action rows, Ticket-gated Orchestrator lifecycle, composer targets, and Intake -> Orchestrator handoff.

Ticket rows currently expose Go/Review/Close/Defer-style actions as display affordances only. To complete the first end-to-end panel before layout/display tuning, the panel needs minimal action dispatch for the user decision points it already displays.

## Goal

Implement simple workspace panel Ticket action dispatch so selected Ticket/action rows can record the intended human decision through the Ticket backend and, where appropriate, notify the workspace Orchestrator.

## Requirements

- Keep the UI/action model thin; do not build a scheduler/queue or a second Ticket state machine.
- Implement minimal dispatch for visible Ticket actions where safe:
  - `Go` / approve Intake-ready Ticket for Orchestrator routing/preflight;
  - `Defer` / move or mark pending where existing Ticket backend semantics support it;
  - `Review` / open or guide to existing review flow if full inline review is not safe in this first slice;
  - `Close` / do not close without a resolution; show a bounded diagnostic or require explicit resolution path.
- Re-check Ticket authority immediately before mutation; do not mutate based only on stale `WorkspacePanelViewModel` data.
- Record actions in `.yoi/tickets` through Rust Ticket APIs / Ticket tools; do not shell out.
- If an Orchestrator is live/reachable and the action is a routing/Go signal, notify it through existing Pod/peer/client mechanisms where feasible.
- No-Ticket workspaces must remain Pod-centric and expose no Ticket actions.
- Preserve existing selected-Pod send/open behavior.
- Keep layout changes minimal; final visual tuning is a later pass.

## Non-goals

- Automatic implementation scheduling.
- Automatic coder/reviewer spawning.
- Full inline review editor.
- Final layout/display tuning.
- Reintroducing `--multi`.

## Acceptance criteria

- Ticket/action rows no longer merely say display-only for every action; at least Go/Defer are actionable or explicitly disabled with a clear reason.
- Action dispatch re-checks current Ticket state before writing.
- Ticket mutations use Rust backend APIs and are visible in `.yoi/tickets` thread/status state.
- Orchestrator notification is attempted for Go/routing actions when feasible and reported as success/warning.
- No-Ticket panel remains functionally equivalent to Pod-centric dashboard.
- Tests cover action dispatch success, stale/invalid action rejection, no-Ticket behavior, and Pod actions remaining intact.
