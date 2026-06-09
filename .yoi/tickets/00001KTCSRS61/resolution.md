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
