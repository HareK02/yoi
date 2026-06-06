---
id: 20260606-060548-workspace-panel-layout-display-tuning
slug: workspace-panel-layout-display-tuning
title: Workspace panel layout and display tuning
status: open
kind: task
priority: P2
labels: [tui, ticket, orchestration, panel, ux]
created_at: 2026-06-06T06:05:48Z
updated_at: 2026-06-06T06:06:30Z
assignee: null
legacy_ticket: null
---

## Background

The first-pass workspace panel implementation is complete. It provides `yoi panel`, Ticket-config-gated Ticket/action rows, no-Ticket Pod-centric fallback, background Orchestrator lifecycle, Companion vs Ticket Intake composer targets, Intake -> Orchestrator handoff, and minimal Ticket action dispatch.

The implementation intentionally kept layout and displayed content simple. The next pass should tune what the user sees and how the panel is organized while preserving the existing TUI visual language and the thin ViewModel/action-dispatch boundaries.

## Goal

Refine the workspace panel layout, labels, ordering, and displayed detail so the first-pass functionality is easy to understand and operate in real use.

## Requirements

- Follow existing TUI visual conventions/components.
- Preserve the single `yoi panel` route and no-Ticket Pod-centric behavior.
- Keep the UI intermediate representation thin; do not move authority into rendering code.
- Improve Ticket/action row labels, status markers, and key hints.
- Improve detail pane content for selected Ticket/action rows:
  - current phase;
  - next action;
  - latest plan/report/review excerpt;
  - related Pods;
  - Orchestrator/handoff/action diagnostics.
- Improve composer target display so Companion vs Ticket Intake is obvious.
- Improve diagnostics/notices for Orchestrator lifecycle, Intake launch/handoff, and Ticket Go/Defer actions.
- Add or refine the phase/dependency/timeline view if it is useful in the first tuning pass.
- Keep Pod-centric no-Ticket panel functionally equivalent to the old multi-Pod dashboard.
- Avoid broad refactors or new scheduling behavior.

## Non-goals

- New scheduling/queue system.
- Automatic implementation authorization.
- Replacing the single-Pod TUI.
- Reintroducing `--multi`.
- Changing Ticket backend storage/config semantics.

## Acceptance criteria

- `yoi panel` remains functional in both Ticket-enabled and no-Ticket workspaces.
- The primary row list makes user-action-required items visually distinguishable from passive background Pods.
- The active composer target is unambiguous.
- Common diagnostics are concise and actionable.
- Layout/display tests or snapshot-like unit tests cover key row/detail rendering decisions where practical.
- Existing validation continues to pass.
