---
id: 20260607-001651-workspace-panel-companion-interface
slug: workspace-panel-companion-interface
title: Workspace panel Companion interface
status: open
kind: task
priority: P1
labels: [tui, panel, companion, orchestration]
workflow_state: intake
created_at: 2026-06-07T00:16:51Z
updated_at: 2026-06-07T00:18:58Z
assignee: null
legacy_ticket: null
---

## Background

The current `yoi panel` `Companion` composer target is a misleading name: it is effectively the selected-Pod direct-send path. The desired UX is different.

The workspace panel should have a real Companion: a workspace-scoped Pod used for the foreground management conversation. It should help the human understand what is done, what remains, what is blocked/queued/in progress, and where intervention is useful. It should not be a direct write-capable worker.

Direct message sending from the panel to arbitrary selected Pods should be removed. The panel composer should talk to the Companion by default, with Ticket Intake as the other explicit creation path in Ticket-enabled workspaces.

## Goal

Redesign the workspace panel composer around a real workspace Companion Pod and remove the selected-Pod direct-send UX.

## Target model

- `yoi panel` restores/spawns a workspace Companion Pod.
- The Companion Pod name is based on the workspace name, matching the default workspace Pod naming direction where practical.
- The panel composer default target is the Companion conversation.
- Ticket Intake remains a separate target for creating/materializing new Ticket requests.
- Selected Pod direct send is removed from the panel.
- Opening/attaching to a Pod remains available for inspecting details.
- The Companion is status-aware but not write-capable.

## Child tickets

1. `workspace-panel-remove-direct-pod-send`
   - Remove selected-Pod direct send from `yoi panel`; keep attach/open and Ticket Intake.

2. `workspace-panel-companion-pod-lifecycle`
   - Restore/spawn the workspace-named Companion Pod and route Companion composer input to it.

3. `companion-status-context-tool-policy`
   - Define Companion prompt/profile/tool policy: status-awareness and human assistance, no direct repository writes or Ticket mutations.

## Non-goals

- Removing Pod attach/open.
- Removing Ticket Intake target.
- Removing Orchestrator lifecycle.
- Giving Companion direct write access or implementation authority.
- Reintroducing `--multi` or `:ticket`.

## Acceptance criteria

- The workspace panel no longer treats `Companion` as selected-Pod direct send.
- The panel has a real workspace Companion Pod/session as the foreground management chat.
- Direct selected-Pod send is removed or disabled with clear UI behavior.
- Companion tool policy prevents direct file/Ticket mutation while still allowing status awareness.
- No-Ticket workspaces still get a useful Companion/Pod-inspection panel.
