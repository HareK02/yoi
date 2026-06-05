# Workspace orchestration panel UI design draft

## Position

The workspace panel should be a successor to the current `--multi` surface, not a two-pane composition of `--multi` plus the normal single-Pod UI. The default unit should be Ticket/action state; Pods are execution/detail resources that the user can attach to on demand.

The panel is not only a Ticket status dashboard. It is a workspace cockpit for reading Ticket state, handling human decision points, launching Intake/Orchestrator actions, and drilling into related Pods when needed.

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

## Visual design

The concrete look-and-feel should follow the existing TUI design language rather than inventing a second product surface. Reuse current terminal UI conventions for borders, list selection, status/action bars, composer placement, key-hint style, transient notices, color/status semantics, and attach/return behavior where possible.

The panel should feel like a more capable `--multi` workspace view, not a new unrelated dashboard. Any new visual treatment for Ticket/action rows or timeline lanes should be a small extension of existing list/detail components, so the single-Pod UI, current multi-Pod view, and workspace panel remain visually coherent.

## State sources and socket boundary

Unlike the normal single-Pod TUI, the panel should not treat a Pod socket transcript as its primary state. The default display can be built from local project records plus lightweight Pod state:

- `.yoi/tickets/` is authoritative for Ticket records, status, thread events, plans, implementation reports, reviews, closures, and umbrella/child relationships.
- Pod metadata/session state is used to show related live/restorable role Pods.
- Visible Pod/client operations are used at action boundaries: restore/spawn Orchestrator, launch Intake, notify/send to a Pod, or attach to a selected Pod.

So the panel itself is local-file-first and does not need to own a long-lived socket as its main state source. It still uses client/socket APIs for operations that actually target live Pods or attach to a transcript.

## UI intermediate representation

Use a thin UI-oriented intermediate representation. It is a render/action-dispatch contract, not a new Ticket backend, scheduler, or state machine.

```text
WorkspacePanelViewModel
- header: workspace label, companion state, orchestrator state, diagnostics
- rows: ordered display rows for user actions, Tickets, and secondary Pod/session entries
- detail: selected item detail blocks
- timeline: optional dependency/phase lanes
- composer: current target, placeholder, send eligibility, hints
```

Rows should carry only display-ready fields plus stable IDs for follow-up actions:

```text
PanelRow
- key: stable selection/action key
- kind: intake | ticket | review | blocked | pod | diagnostic
- title / subtitle
- status marker and priority
- next user action, if any
- related Ticket id/slug, if any
- related Pod names, if any
- disabled reason / key hints
```

Ticket-specific rows can be backed by a compact entry:

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

Keep this intentionally thin:

- Do not duplicate the Ticket backend state machine in the UI.
- Do not load full Markdown thread bodies into the default view model.
- Do not let render functions perform Ticket/Pod I/O.
- Do not execute mutations from stale view-model data alone; action dispatch should re-check the relevant Ticket/Pod authority before mutating.

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

## Action dispatcher boundary

The view model tells the UI what can be displayed and which actions appear available. It should not be the mutation authority. When a user triggers an action, the dispatcher should use the selected stable key to re-read or re-check the relevant authority before acting:

```text
selection key + action intent
→ re-check Ticket / Pod authority
→ run Ticket tool, client method, restore/spawn, send, or attach
→ rebuild view model from local files and Pod state
```

This keeps the panel responsive while avoiding stale UI state becoming a hidden source of truth.

## Timeline / Gantt-like view

A useful first version is a phase/dependency timeline rather than a date-based scheduler:

```text
yoi-local-ticket-backend-migration
├─ ✓ yoi-ticket-cli-parity
├─ ✓ builtin-yoi-local-ticket-backend-config
├─ ✓ migrate-ticket-storage-to-yoi-tickets
└─ ✓ remove-tickets-sh

workspace-orchestration-panel
├─ ▶ workspace-orchestration-panel-design
├─ ○ workspace-panel-orchestrator-lifecycle
├─ ○ workspace-panel-composer-targets
├─ ○ ticket-intake-orchestrator-handoff
└─ ○ workspace-panel-action-model
```

Per-ticket phase lanes can show progress without pretending the system has reliable time estimates:

```text
Ticket        Intake   Preflight   Implement   Review   Close
panel design  ▶        ○           ○           ○        ○
composer      ○        ○           ○           ○        ○
action model  ○        ○           ○           ○        ○
```

This captures what currently matters most: dependency order, gate state, responsible Pods, and human decision points. Duration/ETA scheduling can be added later if the backend starts recording enough authoritative timing data.

## Orchestrator lifecycle

When the workspace panel opens, restore or spawn a workspace Orchestrator named from the workspace directory, e.g. `<dir-name>-orchestrator`. The panel should display its state but should not own its lifetime; closing the panel leaves the Orchestrator running.

Intake Pods receive the Orchestrator handoff/notification target at launch so they can report clarified Ticket readiness without automatically starting implementation. The Orchestrator may prepare routing/preflight, but human Go remains an explicit action.

## Initial implementation boundary

Implement in this order:

1. Define the thin UI intermediate representation (`WorkspacePanelViewModel`, `PanelRow`, `TicketPanelEntry`, composer target/view state, timeline lane rows).
2. Add a local-file-first adapter that builds the view model from `.yoi/tickets/`, role-launch state, visible Pod metadata, and diagnostics.
3. Render the workspace panel using existing TUI visual conventions/components as much as possible.
4. Make the existing multi-Pod panel path consume enough of that model to show action-prioritized Ticket rows.
5. Add composer target selection for Companion vs New Intake.
6. Add action dispatch with re-checks against Ticket/Pod authority before mutation.
7. Add attach/drill-down and return behavior for related Pods.
8. Add the phase/dependency timeline view.

The existing `:ticket ...` commands remain as low-level fallback and debugging affordances, not the primary UX.
