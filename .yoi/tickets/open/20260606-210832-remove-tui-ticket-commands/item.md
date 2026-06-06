---
id: 20260606-210832-remove-tui-ticket-commands
slug: remove-tui-ticket-commands
title: Remove obsolete TUI :ticket commands
status: open
kind: task
priority: P2
labels: [tui, ticket, cleanup, panel]
created_at: 2026-06-06T21:08:32Z
updated_at: 2026-06-06T21:09:49Z
assignee: null
legacy_ticket: null
---

## Background

The old TUI `:ticket ...` command surface was an MVP/fallback for launching fixed Ticket-role Pods before the workspace panel existed. The panel now owns the Ticket/Intake/Orchestrator UX:

- `yoi panel` is the workspace entrypoint.
- Ticket Intake is a composer target.
- Orchestrator lifecycle and Intake handoff are integrated.
- Ticket Go/Defer actions are available from panel rows.
- No-Ticket workspaces remain Pod-centric.

Keeping `:ticket ...` now creates a second user-facing route for the same role-launch concepts and conflicts with the direction that the panel should be the single workspace control surface.

## Goal

Remove the obsolete TUI `:ticket ...` command family and update active docs/tests so users are directed to `yoi panel`, `yoi ticket ...`, Ticket tools, and workflows instead.

## Requirements

- Remove parsing and handling for TUI `:ticket ...` commands:
  - `:ticket intake ...`
  - `:ticket route ...`
  - `:ticket investigate ...`
  - `:ticket implement ...`
  - `:ticket review ...`
- Remove the associated TUI-local `CommandAction::TicketRole` / pending-ticket-role command path if it becomes unused.
- Remove single-Pod runtime handling that launches Ticket roles from `:ticket` commands.
- Keep shared role-launcher code in `client` because `yoi panel` still uses it.
- Do not remove `yoi ticket ...` CLI.
- Do not remove Ticket tools or workflows.
- Do not change `yoi panel` behavior.
- Update active docs/help text that presents `:ticket ...` as a supported route.
- Historical closed Ticket records may still mention `:ticket`; do not mass-rewrite old history.
- Add/adjust tests so `:ticket` is unknown/unsupported and no active parser tests expect it.

## Non-goals

- Removing the Ticket role launcher.
- Removing panel Intake/Orchestrator/Go behavior.
- Removing `yoi ticket ...`.
- Rewriting historical closed Ticket threads/artifacts.
- Layout/display tuning.

## Acceptance criteria

- `:ticket ...` is no longer accepted by TUI command parsing.
- There is no runtime path in the single-Pod TUI that launches Ticket roles from `:ticket`.
- Active docs/help no longer direct users to `:ticket`.
- `yoi panel` and panel Intake launch still work.
- `yoi ticket ...` still works.
- Tests cover removal or unknown-command behavior.
