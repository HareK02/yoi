---
title: "Remove workspace panel direct Pod send"
state: "closed"
created_at: "2026-06-07T00:16:51Z"
updated_at: "2026-06-07T02:00:58Z"
---

## Background

The panel currently uses `Companion` as the default composer target, but implementation-wise this is selected-Pod direct send. That is no longer desired.

The panel should not provide an arbitrary direct-message path to selected Pods. Users can attach/open Pods to inspect details, and Ticket Intake remains the path for new work requests. A real workspace Companion Pod will become the foreground management chat in a follow-up ticket.

## Goal

Remove selected-Pod direct send from `yoi panel` and stop presenting it as Companion behavior.

## Requirements

- Remove or disable the panel code path that sends composer text directly to the selected Pod via `Method::Run`.
- Remove UI labels/key hints that imply selected-Pod direct send is supported.
- Preserve Pod attach/open behavior for inspection.
- Preserve Ticket Intake composer target and launch behavior.
- Preserve Orchestrator lifecycle and Ticket action dispatch.
- No-Ticket workspaces should remain useful for Pod discovery/attach/open even without direct send.
- If composer input cannot yet be routed to a real Companion in this ticket, show a clear bounded diagnostic instead of falling back to selected-Pod direct send.
- Do not reintroduce `--multi` or `:ticket`.

## Non-goals

- Implementing real Companion Pod lifecycle; `workspace-panel-companion-pod-lifecycle` owns that.
- Companion prompt/profile/tool policy; `companion-status-context-tool-policy` owns that.
- Removing attach/open.
- Removing Ticket Intake.

## Acceptance criteria

- Panel composer text is no longer sent directly to arbitrary selected Pods.
- Existing tests that assumed selected-Pod direct send are updated or removed.
- Pod attach/open still works.
- Ticket Intake still works.
- UI/key hints do not advertise direct Pod send.
