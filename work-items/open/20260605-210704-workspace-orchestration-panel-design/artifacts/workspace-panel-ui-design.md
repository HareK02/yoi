# Workspace orchestration panel UI design draft

## Position

The workspace panel should be a successor to the current `--multi` surface, not a two-pane composition of `--multi` plus the normal single-Pod UI. The default unit should be Ticket/action state; Pods are execution/detail resources that the user can attach to on demand.

## UI shape

```text
Workspace Orchestration Panel
├─ header: workspace, orchestrator state, companion state
├─ primary list: user-action-required Tickets / Intakes / Reviews
├─ detail pane: selected Ticket plan, current phase, related Pods, latest reports
├─ optional timeline/plan view: umbrella/child dependency lanes and phase progress
└─ composer: explicit target selector
```

The main list must not be a raw Pod list. It should sort by user decision priority:

1. Intake needs clarification or approval.
2. Ticket is ready for human Go.
3. Review or close decision is required.
4. Ticket is blocked by an explicit human/project decision.
5. Active implementation/review work.
6. Informational Pod status and restorable historical sessions.

## Relationship to `--multi` and normal TUI

Reuse the useful `--multi` behaviors:

- discover visible live/restorable Pods;
- show live/stopped/working state;
- choose a send/attach target;
- attach to a Pod/session and return to the workspace panel;
- keep background Pods alive when the panel exits.

Do not embed the normal single-Pod UI as a permanent second pane. The panel should drill into a selected Pod/session only when the user explicitly attaches. This avoids maintaining two simultaneous chat surfaces and keeps normal Pod history ownership unchanged.

## Composer target model

The composer has an explicit target selector:

- `Companion`: management chat for this workspace panel session.
- `New Intake`: send the composer body as the initial user input of a new Ticket Intake Pod.
- `Selected Ticket`: record a comment/decision/Go/review-style action against the selected Ticket, usually via Ticket tools or an Orchestrator notification.
- `Selected Pod`: advanced/direct send or attach path, not the main workflow.

Dynamic user text must be committed to the destination authority:

- Companion text goes to Companion history.
- New Intake text goes to the Intake Pod initial user input/history.
- Ticket decisions/comments go to Ticket records and/or the Pod that receives the actual message.
- Nothing should be injected only into hidden context.

## Ticket-centric detail model

Each visible Ticket row should be backed by a plain data entry, not by direct render-time Pod inspection:

```text
TicketPanelEntry
- id / slug / title / status
- current phase: intake | preflight | implementing | reviewing | close_ready | blocked | closed
- next user action: clarify | go | review | close | wait | none
- parent/child relationship
- related role Pods and their live/restorable state
- latest plan / implementation report / review summary excerpts
- diagnostics and blocked reason
```

This model should be assembled from the Ticket backend, Ticket thread entries, known role launches, visible Pod metadata, and Orchestrator/Intake handoff records. The UI should consume this plain model so later views can be added without coupling rendering to Pod internals.

## Timeline / Gantt-like view

A useful first version is a phase/dependency timeline rather than a date-based scheduler:

```text
yoi-local-ticket-backend-migration
├─ ✓ yoi-ticket-cli-parity
├─ ▶ builtin-yoi-local-ticket-backend-config
├─ ○ migrate-ticket-storage-to-yoi-tickets
└─ ○ remove-tickets-sh

workspace-orchestration-panel
├─ ○ workspace-orchestration-panel-design
├─ ○ workspace-panel-orchestrator-lifecycle
├─ ○ workspace-panel-composer-targets
├─ ○ ticket-intake-orchestrator-handoff
└─ ○ workspace-panel-action-model
```

Per-ticket phase lanes can show progress without pretending the system has reliable time estimates:

```text
Ticket        Intake   Preflight   Implement   Review   Close
backend cfg   ✓        ✓           ▶           ○        ○
storage move  ✓        ○           ○           ○        ○
panel design  ▶        ○           ○           ○        ○
```

This captures what currently matters most: dependency order, gate state, responsible Pods, and human decision points. Duration/ETA scheduling can be added later if the backend starts recording enough authoritative timing data.

## Orchestrator lifecycle

When the workspace panel opens, restore or spawn a workspace Orchestrator named from the workspace directory, e.g. `<dir-name>-orchestrator`. The panel should display its state but should not own its lifetime; closing the panel leaves the Orchestrator running.

Intake Pods receive the Orchestrator handoff/notification target at launch so they can report clarified Ticket readiness without automatically starting implementation. The Orchestrator may prepare routing/preflight, but human Go remains an explicit action.

## Initial implementation boundary

Implement in this order:

1. Build the plain `WorkspacePanelModel` / `TicketPanelEntry` / `UserActionEntry` model.
2. Make the existing multi-Pod panel consume enough of that model to show action-prioritized Ticket rows.
3. Add composer target selection for Companion vs New Intake.
4. Add attach/drill-down and return behavior for related Pods.
5. Add the phase/dependency timeline view.

The existing `:ticket ...` commands remain as low-level fallback and debugging affordances, not the primary UX.
