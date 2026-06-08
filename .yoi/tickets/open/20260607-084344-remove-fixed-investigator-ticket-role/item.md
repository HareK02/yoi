---
id: 20260607-084344-remove-fixed-investigator-ticket-role
slug: remove-fixed-investigator-ticket-role
title: Remove fixed investigator Ticket role
status: open
kind: task
priority: P2
labels: [ticket, orchestration, role, cleanup]
workflow_state: 'inprogress'
created_at: 2026-06-07T08:43:44Z
updated_at: '2026-06-08T11:16:58Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T11:15:16Z'
---

## Background

`investigator` was introduced by prior AI-driven Ticket orchestration design as a fixed Ticket role alongside `intake`, `orchestrator`, `coder`, and `reviewer`. It was not part of the current desired role profile set, and the former TUI `:ticket investigate` surface has already been removed.

Investigation/read-only research remains useful, but it should be modeled as a task-specific helper Pod spawned by Intake/Orchestrator/planning when needed, not as a permanent fixed Ticket role with workspace config/profile slots.

## Goal

Remove `investigator` as a fixed Ticket role and clean up user/project-facing references while preserving the ability for Orchestrator/planning flows to spawn read-only investigation helper Pods as ordinary task-specific Pods.

## Requirements

- Remove `TicketRole::Investigator` from the fixed role model.
- Update Ticket config parsing/scaffold/defaults so `.yoi/ticket.config.toml` only requires/advertises:
  - `intake`
  - `orchestrator`
  - `coder`
  - `reviewer`
- Treat `[roles.investigator]` in current config according to the project’s desired compatibility policy; prefer a clear config error or documented migration over silently using an unsupported role.
- Update launcher/client/TUI tests and prompt generation that currently mention or branch on `Investigator`.
- Remove docs/workflow wording that presents `investigator` as a fixed role or configured role slot.
- Preserve wording/behavior that allows Orchestrator/planning to create read-only helper Pods for investigation when explicitly useful.
- Do not reintroduce the removed TUI `:ticket investigate` command surface.
- Do not add a generic arbitrary role registry as part of this cleanup.

## Acceptance criteria

- No runtime fixed-role enum/config/scaffold path exposes `investigator` as a Ticket role.
- `yoi ticket init` no longer emits `[roles.investigator]`.
- Project docs describe investigation as an optional helper-Pod activity under Intake/Orchestrator/planning, not a configured Ticket role.
- Existing Ticket role launch flows for intake/orchestrator/coder/reviewer still work.
- Focused tests for `ticket::config`, `client::ticket_role`, and any affected TUI role-launch/panel paths pass.
- `target/debug/yoi ticket doctor`, `cargo fmt --check`, and `git diff --check` pass.
