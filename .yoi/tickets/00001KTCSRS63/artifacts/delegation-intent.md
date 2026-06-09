# Delegation intent: workspace panel composer targets

## Classification

`implementation-ready` after action model and Orchestrator lifecycle slices.

The panel now has `yoi panel`, Ticket-config-gated Ticket UI, Pod-centric no-Ticket behavior, and Ticket-enabled Orchestrator lifecycle. This ticket adds the first composer target split so users can send either to the Companion/selected Pod path or directly to a new Intake role Pod.

## Intent

Add workspace panel composer target selection with two first-class targets:

- `Companion` / current Pod-centric send path;
- `Ticket Intake` / launch a new Intake role Pod with the composer body as its first run input.

The Intake path must not append the message to Companion/current Pod history. It should go only to the launched Intake Pod's initial `Method::Run` input/history through the existing Ticket role launcher path.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/workspace-panel-composer-targets`
- branch: `work/workspace-panel-composer-targets`

This ticket may read tracked `.yoi/tickets` records/design artifacts. Do not read or edit `.yoi/memory/`.

## Requirements

- Add a workspace panel composer target model with at least:
  - `Companion` / existing selected-Pod send behavior;
  - `Ticket Intake`.
- The UI must clearly show the active composer target using existing TUI visual conventions.
- Add a simple keybinding or command path to switch targets without losing typed text where practical.
- `Companion` target should preserve existing panel/Pod-centric send behavior.
  - In no-Ticket workspace, this should be the only available target and should behave like the current Pod dashboard send.
- `Ticket Intake` target should be available only when Ticket config is defined/usable.
- Empty Intake messages must be rejected with a bounded diagnostic/actionbar-style message.
- Intake target must call the existing Ticket role launcher (`TicketRole::Intake`) rather than constructing spawn/profile/workflow prompt content inside TUI.
- The composer body must become dynamic user instruction/run input for the Intake launch.
- The Intake body must not be sent to Companion/current Pod history or to any selected Pod.
- After a successful Intake launch, clear the composer and surface a bounded success notice including the launched Pod name.
- On launch failure, keep the composer text and surface a bounded diagnostic.
- Do not implement Intake -> Orchestrator handoff payload in this ticket; that is the next child ticket.
- Do not reintroduce `--multi`.
- Keep layout changes minimal; final display tuning is later.

## Non-goals

- Intake -> Orchestrator handoff contract.
- Generic arbitrary target routing.
- Scheduler/queue.
- Ticket action queue display.
- Final layout/display tuning.
- Companion Pod/session creation beyond preserving existing panel send behavior.

## Current code map

- `crates/tui/src/multi_pod.rs`
  - Current panel substrate, composer handling, selected Pod send/open behavior, notices/diagnostics.
- `crates/tui/src/workspace_panel.rs`
  - ViewModel/header/diagnostics. Extend with composer target view state only if useful; keep it thin.
- `crates/client/src/ticket_role.rs`
  - `TicketRoleLaunchContext`, `TicketRole::Intake`, and `launch_ticket_role_pod(...)`. Use this path for Intake launch.
- `crates/tui/src/command.rs`
  - Existing `:ticket intake ...` route for reference only; do not duplicate prompt/profile construction in TUI.
- `crates/tui/src/app.rs`
  - Normal composer/history semantics for reference; ensure Intake target does not mutate the wrong history.

## Validation

Run at least:

- targeted TUI tests for composer target switching and Enter behavior;
- a test that Intake target rejects empty input;
- a test that Intake target calls/constructs a Ticket role launch request without sending to selected Pod path;
- a test that no-Ticket workspace exposes only Companion/Pod-centric send;
- `cargo test -p tui workspace_panel`;
- `cargo test -p tui multi` or updated panel test names;
- `cargo test -p client ticket_role` if role launcher code changes;
- `cargo test -p yoi panel`;
- `cargo check --workspace --all-targets`;
- `cargo fmt --check`;
- `git diff --check`;
- `cargo build -p yoi`;
- `target/debug/yoi ticket doctor`.

Run `nix build .#yoi --no-link` if feasible.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- composer target model/keybinding;
- no-Ticket target behavior;
- Intake launch path and how the typed body is routed;
- proof that Intake body does not go to Companion/current Pod history;
- diagnostics/success notices;
- tests updated/added;
- validation results;
- known follow-up layout/display tuning items.
