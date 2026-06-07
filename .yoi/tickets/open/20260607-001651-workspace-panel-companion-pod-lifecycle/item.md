---
id: 20260607-001651-workspace-panel-companion-pod-lifecycle
slug: workspace-panel-companion-pod-lifecycle
title: Workspace panel Companion Pod lifecycle
status: open
kind: task
priority: P2
labels: [tui, panel, companion, pod]
workflow_state: ready
created_at: 2026-06-07T00:16:51Z
updated_at: 2026-06-07T10:18:01Z
assignee: null
legacy_ticket: null
---

## Background

The panel needs a real Companion Pod rather than treating `Companion` as selected-Pod direct send. The Companion is the foreground workspace management chat: the place where the human asks what is happening, what is done, what remains, and where attention is useful.

The desired identity is a workspace-named Pod, similar to the default Pod created from a workspace name when starting normally. This should make `yoi panel` feel like opening the workspace's management conversation rather than a raw Pod dashboard.

## Goal

Restore or spawn a workspace-named Companion Pod when `yoi panel` opens, and route the panel Companion composer target to that Pod.

## Requirements

- Define the Companion Pod naming rule.
  - Use the workspace directory basename as the Companion Pod id/name.
  - For this repository, the Companion Pod id/name is `yoi`.
  - Do not add a `-companion` suffix or introduce a separate companion-specific id for the normal workspace Companion.
  - This intentionally lets `yoi panel` treat an existing default workspace Pod named `yoi` as the Companion when it is live/restorable.
  - Avoid colliding with the workspace Orchestrator name, which remains distinct (for example `yoi-orchestrator`).
- On `yoi panel` open:
  - if Companion is live, use it;
  - if restorable, restore it;
  - if missing, spawn it with the Companion profile/prompt/policy from the companion tool-policy ticket;
  - surface bounded diagnostics if unavailable.
- Panel close must not stop the Companion.
- The panel composer `Companion` target sends user text to the Companion Pod history, not to arbitrary selected Pods.
- The Companion target is available in both Ticket-enabled and no-Ticket workspaces.
- Ticket Intake remains a separate target in Ticket-enabled workspaces and must not append Intake text to Companion history.
- Preserve Pod attach/open for inspection.
- Avoid blocking panel redraw longer than necessary; coordinate with `workspace-panel-nonblocking-transitions` if needed.
- Do not give the Companion broad write scope by default.

## Non-goals

- Direct selected-Pod send.
- Orchestrator scheduling semantics.
- Ticket Intake handoff changes beyond preserving separation.
- Tool/prompt policy details beyond consuming the agreed Companion profile/policy.
- Reintroducing `--multi` or `:ticket`.

## Acceptance criteria

- `yoi panel` has a real Companion Pod identity and lifecycle.
- Companion composer messages are committed to the Companion Pod's history.
- No-Ticket workspaces still have a Companion management chat plus Pod attach/open.
- Ticket Intake messages do not enter Companion history.
- Companion remains running after panel exits.
