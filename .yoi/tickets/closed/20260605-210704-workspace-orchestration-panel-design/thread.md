<!-- event: create author: yoi ticket at: 2026-06-05T21:07:04Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-05T21:22:49Z -->

## Plan

Recorded an initial UI design draft from the panel discussion in `artifacts/workspace-panel-ui-design.md`.

Key direction:
- make the workspace panel a successor to `--multi`, not a permanent split with the normal single-Pod UI;
- use Ticket/action state as the default unit and attach to Pod/session details only on demand;
- provide an explicit composer target selector for Companion / New Intake / Selected Ticket / advanced Selected Pod;
- add a Gantt-like phase/dependency timeline based on Ticket gates and umbrella/child ordering rather than date estimates.

This is a design draft for review/iteration, not implementation approval for the child panel tickets yet.


---

<!-- event: decision author: hare at: 2026-06-05T21:32:54Z -->

## Decision

Updated the workspace panel design draft per UI direction:

- concrete look-and-feel should follow the existing TUI design language/components rather than introducing a separate dashboard style;
- the panel should remain a `--multi` successor with attach/drill-down, not a permanent split with the normal single-Pod UI;
- rendering should go through a UI intermediate representation (`WorkspacePanelViewModel`, `TicketPanelEntry`, action rows, composer view state, timeline lanes) built from Ticket/Pod/runtime sources before widgets render.

The design ticket acceptance criteria now explicitly require both the UI intermediate representation and existing-TUI visual convention constraints.


---

<!-- event: decision author: hare at: 2026-06-05T22:29:53Z -->

## Decision

Updated the workspace panel design to keep the UI intermediate representation intentionally thin.

Direction recorded:
- the panel is local-file-first for display: `.yoi/tickets/` is the Ticket authority, with Pod metadata/session state only for related live/restorable status;
- sockets/client APIs are used at operation boundaries such as spawn/restore/send/attach, not as the panel's primary state source;
- the intermediate representation is a render/action-dispatch contract (`WorkspacePanelViewModel`, rows, compact Ticket entries), not a second Ticket backend, scheduler, or UI-owned state machine;
- the panel is not display-only: it is a Ticket-centric workspace cockpit for status, decision points, Intake/Orchestrator actions, and Pod drill-down.


---

<!-- event: review author: hare at: 2026-06-05T22:35:18Z status: approve -->

## Review: approve

User approved the current workspace panel design direction and asked to implement a first end-to-end version before detailed layout/display tuning.

Approved implementation direction:
- follow the existing TUI visual language/components;
- keep the UI intermediate representation thin and simple;
- make panel display local-file-first from `.yoi/tickets/`;
- use client/socket APIs only at live operation boundaries such as spawn/restore/send/attach;
- build the first pass broadly enough to exercise the whole flow, then refine layout and displayed details afterward.


---

<!-- event: close author: hare at: 2026-06-05T22:35:18Z status: closed -->

## Closed

Completed the workspace orchestration panel design.

Final direction:
- The panel is a `--multi` successor, not a permanent split with the normal single-Pod TUI.
- The concrete look follows existing TUI conventions/components.
- The main unit is Ticket/action state; Pods are secondary execution/detail resources opened via attach/drill-down.
- Display is local-file-first from `.yoi/tickets/` plus lightweight Pod metadata/session state.
- Client/socket APIs are used at live operation boundaries: Orchestrator restore/spawn, Intake launch, send/notify, attach.
- The UI intermediate representation is intentionally thin: `WorkspacePanelViewModel`, `PanelRow`, `TicketPanelEntry`, composer state, and timeline lanes as render/action-dispatch data, not a second Ticket backend, scheduler, or state machine.
- The UI is not display-only; it is a Ticket-centric workspace cockpit for status, human decision points, Intake/Orchestrator actions, and Pod drill-down.
- First implementation should cover the end-to-end structure before detailed layout/display tuning.

Design artifact:
- `artifacts/workspace-panel-ui-design.md`

The implementation child tickets can proceed.


---
