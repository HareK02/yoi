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

<!-- event: decision author: hare at: 2026-06-05T23:13:39Z -->

## Decision

Decision/update from launch and no-Ticket workspace discussion:

Remove `--multi` as a user-facing launch option rather than keeping it as a compatibility alias.

Target launch model:

- `yoi panel` opens the workspace panel for the current working directory/workspace.
- The panel resolves Ticket config from the workspace.
- If `.yoi/ticket.config.toml` / Ticket config is defined and usable, the panel shows Ticket/action-centric workspace UI.
- If Ticket config is undefined, the panel suppresses Ticket-related UI entirely:
  - no Ticket/action rows;
  - no Ticket-specific Pod labels/actions;
  - no Intake/Orchestrator/Ticket workflow affordances;
  - no Ticket-related diagnostics unless needed to explain an explicitly requested Ticket action.
- In that no-Ticket mode, `yoi panel` should provide the same functional surface as the current `--multi` view: Pod discovery/status, selection, attach/open, and direct send where currently supported.

This keeps one panel implementation while avoiding a permanent `--multi` surface or confusing Ticket UI in workspaces that have not opted into Ticket config.


---

<!-- event: close author: hare at: 2026-06-06T06:05:38Z status: closed -->

## Closed

Completed the first-pass workspace orchestration panel implementation.

Delivered child/follow-up tickets:
- `workspace-orchestration-panel-design`: approved design; panel is a unified `yoi panel` workspace view, not a `--multi`/single-Pod split.
- `workspace-panel-action-model`: added thin local-file-first workspace panel ViewModel, Ticket/action rows, no-Ticket Pod-centric fallback, `yoi panel`, and removed `--multi`.
- `workspace-panel-orchestrator-lifecycle`: Ticket-enabled panels ensure/restore/spawn a background workspace Orchestrator and skip Orchestrator lifecycle in no-Ticket workspaces.
- `workspace-panel-composer-targets`: added Companion vs Ticket Intake composer targets and Intake launch through the role launcher.
- `ticket-intake-orchestrator-handoff`: added auditable Intake -> Orchestrator handoff and pre-run peer registration.
- `workspace-panel-ticket-action-dispatch`: added safe dispatch for Go/Defer and safe diagnostics for Review/Close.

Final first-pass behavior:
- `yoi panel` opens the workspace panel for cwd/workspace.
- `--multi` is removed as a user-facing route.
- If `.yoi/ticket.config.toml` is absent, the panel suppresses Ticket UI and behaves as a Pod-centric dashboard.
- If Ticket config is usable, the panel shows Ticket/action rows, background Pod rows, Orchestrator lifecycle state, and composer target selection.
- Ticket Intake composer target launches Intake with the typed message as Intake input, not Companion/current Pod history.
- Intake receives Orchestrator handoff in committed launch input/history and pre-run peer registration is attempted.
- Go records a Ticket decision for Orchestrator routing/preflight and optionally notifies Orchestrator, without starting implementation.
- Defer records a decision and moves open Tickets to pending where applicable.
- Review/Close remain safe explicit-flow diagnostics rather than silent approval/closure.
- Layout/display polish is intentionally deferred to the next pass.

Validation across final slices included:
- `cargo test -p tui workspace_panel`
- `cargo test -p tui multi_pod`
- `cargo test -p client ticket_role`
- `cargo test -p ticket`
- `cargo test -p yoi panel`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link`


---
