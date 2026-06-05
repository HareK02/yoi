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
