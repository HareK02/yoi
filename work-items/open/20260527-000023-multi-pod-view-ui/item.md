---
id: 20260527-000023-multi-pod-view-ui
slug: multi-pod-view-ui
title: Multi-Pod view UI
status: open
kind: task
priority: P2
labels: [tui, pod]
created_at: 2026-05-27T00:00:23Z
updated_at: 2026-05-28T15:47:20Z
assignee: null
legacy_ticket: null
---

## Background

This work item was migrated from an unfinished TODO.md entry that did not have a dedicated legacy ticket file.

The direction is to make TUI capable of treating multiple Pods as first-class targets instead of forcing the operator to attach/open one Pod at a time before sending input. The main view should be able to show live Pods by status, show stopped Pod history entries, and keep an editable composer available while the user moves selection across Pods.

This ticket is downstream of the shared TUI Pod list/view abstraction. The concrete multi-Pod view requirements should be defined after the common list/view model exists, so this ticket can focus on view switching and interaction policy rather than inventing another Pod list representation.

## Prerequisite

- `20260528-141602-tui-pod-list-view-abstraction`

## CLI entrypoint

- Add `tui --multi` as the explicit entrypoint for the multi-Pod dashboard.
- Do not change `tui -r` / `tui --resume` semantics; those remain the resume picker.
- Do not add a short `-m` alias yet.
- `--multi` conflicts with direct single-Pod/session selectors for this ticket:
  - positional pod name
  - `--pod <name>`
  - `--session <UUID>`
  - `-r` / `--resume`
  - `--socket`
- Initial selected Pod for `--multi --pod <name>` is out of scope; add it later if the UX needs it.

## Current implementation notes

Current TUI is essentially single-Pod oriented:

- `crates/tui/src/main.rs` starts one `PodClient` and the event loop sends composer input to that attached Pod.
- `App` owns one conversation/history view, one composer, and one local queued-input state for the currently attached Pod.
- The existing picker can list/restore/attach Pods, but choosing an entry transitions the TUI into that Pod rather than keeping a multi-Pod dashboard active.
- Live/stopped Pod discovery already exists around picker/discovery code, and should be reused through the prerequisite abstraction rather than duplicated in this ticket.

Because of this, multi-Pod view should be designed as a new TUI mode/state over the shared Pod list abstraction, not as a small tweak to the current single attached-Pod event loop.

## Desired UX direction

The multi-Pod view should center on a Pod list and a persistent composer:

- Live Pods are grouped or visibly categorized by status.
  - waiting / idle Pods: ready to receive input.
  - working / running Pods: currently processing; input should not be sent as another immediate `Method::Run` unless the protocol can accept it.
  - paused Pods: distinguish from both idle and working.
- Stopped Pods are shown as history/restorable entries.
  - They are visible for review/restore/open actions.
  - Direct message send is disabled until an explicit restore/attach/create flow exists for that entry.
- The text area/composer remains visible and retains its contents while the selected Pod changes.
- The selected Pod is the current send target.
  - The UI must show the target Pod name/status near the composer so a message cannot be sent to the wrong Pod silently.
  - Sending to an idle live Pod should be possible without opening/attaching that Pod as the main conversation view.
  - Sending should clear the composer only after delivery is accepted or otherwise reported as queued according to the rule below.
- For a working/running Pod, the initial behavior should be conservative.
  - Do not blindly issue `Method::Run` and surface `AlreadyRunning` as normal UX.
  - Either disable direct send with an actionbar diagnostic, or implement target-specific local queueing that sends when that Pod becomes idle.
  - If queueing is implemented, queues must be per-Pod, visibly attached to the target, and should not reuse the current single-Pod composer queue implicitly.

## Requirements

- Add the `tui --multi` CLI entrypoint and reject conflicting single-Pod/session selectors.
- Build on the completed `tui-pod-list-view-abstraction` for row/state/source modeling.
- Add or design a TUI mode for multi-Pod view that can show:
  - live idle/waiting Pods.
  - live working/running Pods.
  - paused Pods.
  - stopped/restorable Pod history entries.
- Preserve a composer/text area while the selection changes.
- Support direct send to the selected idle live Pod without switching the whole TUI into that Pod view.
  - Delivery must use the same safety expectations as other socket send paths: no fire-and-forget success, and no connect-time `Alert` / `Snapshot` deadlock.
  - Failed delivery must leave the text in the composer or an explicit per-target queue.
- Define interaction for non-idle targets.
  - running: disabled or per-target queued.
  - paused: resume/continue action is separate from normal send unless protocol semantics are explicitly defined.
  - stopped: restore/open action is separate from send.
- Keep the single-Pod conversation view available.
  - Opening/attaching a selected Pod remains an explicit action.
  - Direct send from multi-Pod view must not imply that the selected Pod's full history is now loaded as the main conversation view.
- Avoid host-wide visibility expansion.
  - The list source must be explicit and must respect the visibility model decided by the prerequisite ticket.

## Acceptance criteria

- `tui --multi` starts the multi-Pod view, and conflicting CLI argument combinations are rejected with clear errors.
- Multi-Pod view requirements are implemented against the shared Pod list/view abstraction, not a separate list model.
- The view can render live Pods with idle/running/paused distinctions and stopped/restorable history entries.
- A persistent composer remains available while moving selection.
- Sending from the composer targets the selected idle live Pod without opening it as the main conversation view.
- Non-idle and stopped targets have explicit, safe UX behavior.
- Delivery failure does not lose user input.
- The UI clearly indicates the selected send target and status.
- Existing single-Pod TUI attach/resume behavior continues to work.
- Tests cover selection-to-target mapping, disabled/queued non-idle behavior, and composer preservation across selection changes.
- `cargo fmt --check`
- `cargo check -p tui -p client -p pod`
- Relevant focused tests for TUI state/model behavior.

## Out of scope

- Implementing the prerequisite Pod list/view abstraction itself.
- Child Pod panel completion (`20260527-000017-tui-spawned-pod-panel`).
- Host-wide Pod browser.
- Changing Pod visibility, permission, registry, or discovery authority.
- Protocol changes for accepting concurrent user messages while a Pod is already running.
- Native GUI.
