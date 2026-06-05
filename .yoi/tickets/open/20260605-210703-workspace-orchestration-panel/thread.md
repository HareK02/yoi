<!-- event: create author: yoi ticket at: 2026-06-05T21:07:03Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-05T21:09:19Z -->

## Plan

Plan: design the workspace orchestration panel before implementation.

The panel is not a `:ticket` command extension. It is a workspace-scoped orchestration surface with:

- Companion foreground management chat;
- Ticket Intake composer target that sends user input directly to Intake, not Companion history;
- background workspace Orchestrator restored/spawned as `<dir-name>-orchestrator`;
- Intake -> Orchestrator handoff;
- an action model prioritizing human-required Ticket/Intake decisions over raw Pod idle state.

Implementation is split into child tickets:

1. `workspace-orchestration-panel-design`
2. `workspace-panel-orchestrator-lifecycle`
3. `workspace-panel-composer-targets`
4. `ticket-intake-orchestrator-handoff`
5. `workspace-panel-action-model`

Existing `:ticket ...` commands remain as low-level fallback, not the main UX.


---

<!-- event: decision author: hare at: 2026-06-05T23:05:06Z -->

## Decision

Decision/update from UI discussion:

Do not preserve `--multi` as a separate long-term UI surface. The workspace panel should become the unified workspace view.

`--multi`-style Pod list behavior remains useful, but should be treated as the panel's no-Ticket / Pod-centric route rather than a distinct architecture. In workspaces where Ticket config/storage is unavailable or Ticket usage is not desired, opening the panel should still provide the current multi-Pod discovery/attach/send affordances as a fallback view.

Implications:
- Implementation can reuse current `--multi` code/components as the first panel substrate.
- The long-term command/launch model should prefer a panel view option; any existing `--multi` entry point can remain as a compatibility alias or shortcut into the Pod-centric panel route during migration.
- The panel model should tolerate an empty/unavailable Ticket backend and still show useful Pod rows/actions.
- Avoid designing two separate surfaces with duplicated selection, attach, send, and rendering logic.


---

<!-- event: decision author: hare at: 2026-06-05T23:08:49Z -->

## Decision

Decision/update from launch-route discussion:

Current route is still `yoi --multi` -> `LaunchMode::Multi` -> `single_pod::run_multi(...)` -> current multi-Pod dashboard. There is no dedicated `yoi panel` command yet.

Target route should be `yoi panel`: open the workspace panel for the current working directory/workspace. The panel should resolve the workspace from cwd using the same project/config/Ticket root conventions as the rest of Yoi, then:

- show Ticket/action-centric mode when `.yoi/tickets` / Ticket config is available or Ticket use is active;
- fall back to the Pod-centric route (current `--multi` behavior) when the workspace has no Ticket backend/config or no Ticket records;
- keep current `--multi` as a temporary compatibility alias/shortcut into `yoi panel`'s Pod-centric route during migration, rather than a separate long-term surface.

This keeps a single workspace control surface while preserving the existing multi-Pod affordances for non-Ticket workspaces.


---
