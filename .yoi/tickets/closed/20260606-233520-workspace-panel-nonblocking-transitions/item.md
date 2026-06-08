---
id: 20260606-233520-workspace-panel-nonblocking-transitions
slug: workspace-panel-nonblocking-transitions
title: Make workspace panel transitions non-blocking
status: 'closed'
kind: task
priority: P2
labels: [tui, panel, ux, performance]
created_at: 2026-06-06T23:35:20Z
updated_at: '2026-06-08T02:29:21Z'
assignee: null
legacy_ticket: null
workflow_state: 'done'
queued_by: 'workspace-panel'
queued_at: '2026-06-08T00:02:21Z'
---

## Background

`yoi panel` currently feels delayed when opening the panel, attaching from the panel to a Pod, and returning from an attached Pod back to the panel.

The delay is not waiting for an LLM response. It is mostly waiting for synchronous local/runtime work before the next screen is drawn:

- initial panel load waits for a full panel snapshot;
- Ticket-enabled panel load can ensure/observe the workspace Orchestrator before first draw;
- opening a Pod waits for socket connect or restore/spawn before visible transition;
- returning from a nested Pod waits for a synchronous panel reload before showing the panel again.

The panel already has a background `PendingReload` mechanism while it is visible, but the open/attach/return transition paths still block on reload/connect work before changing the visible screen.

## Goal

Make workspace panel transitions feel immediate by showing the next relevant screen/state first and moving snapshot reload / Pod connection / Orchestrator observation work into background or explicit progress states where practical.

## Problem points

Current important paths:

```text
yoi panel
→ MultiPodApp::load(...)
→ load_multi_pod_snapshot(..., OrchestratorLifecycleMode::Ensure)
→ first draw only after snapshot/ensure completes
```

```text
panel Enter on Pod
→ prepare_open()
→ run_pod_name_nested(...)
→ try_connect_live_pod / restore/spawn
→ single-Pod screen only after connect/restore is ready
```

```text
return from attached Pod
→ app.finish_open(...)
→ app.reload_or_notice().await
→ panel draw only after reload completes
```

## Requirements

- Returning from an attached Pod should redraw the previous panel immediately, with a clear refreshing notice, then perform snapshot reload in the background.
- Avoid awaiting `app.reload_or_notice().await` on the attach-return path before the panel is visible.
- Opening/attaching a Pod should visibly transition to an attaching/restoring progress state before waiting on socket connect or restore/spawn.
- Initial `yoi panel` open should avoid blocking first draw on non-essential refresh/ensure work where practical.
  - A minimal/loading panel or last/current partial snapshot is acceptable.
  - Orchestrator ensure can be surfaced as background progress/diagnostic if it cannot be completed before first draw cheaply.
- Preserve correctness: background reload must still update Pod state, Ticket rows, Orchestrator state, and diagnostics when it completes.
- Preserve no-Ticket Pod-centric behavior.
- Do not lose key input during transition/progress states.
- Do not introduce duplicate overlapping reload tasks; reuse/extend `PendingReload` or similar guard.
- Do not change Ticket workflow semantics or Pod restore/spawn authority.

## Non-goals

- Changing Ticket workflow state.
- Changing Orchestrator scheduling semantics.
- Rewriting the whole TUI runtime loop.
- Replacing the single-Pod TUI.
- Cosmetic layout tuning unrelated to transition latency.

## Acceptance criteria

- Returning from a nested Pod displays the panel before panel snapshot reload completes.
- Panel shows a concise `refreshing` / `attaching` / `restoring` style notice during background work.
- Background reload completion updates the panel without dropping selection/composer text unnecessarily.
- Attach/open path gives visible feedback before slow socket connect/restore work.
- Tests cover the non-blocking return/reload behavior where practical.
- Existing validation for panel, Pod-centric mode, and Ticket-enabled mode continues to pass.
