# Delegation intent: workspace panel Ticket action dispatch

## Classification

`implementation-ready` as the final first-pass panel slice before layout/display tuning.

The panel now supports `yoi panel`, Ticket/action display, Ticket-gated Orchestrator lifecycle, composer targets, and Intake -> Orchestrator handoff. Ticket action rows are still mostly display-only. This ticket should make the core human decision affordances actionable without creating a scheduler.

## Intent

Implement minimal safe Ticket action dispatch from selected workspace panel Ticket rows. The first pass should prioritize `Go` / Intake-approved routing and `Defer`; other displayed actions may be actionable if safe or remain explicitly disabled with a clear bounded diagnostic.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/workspace-panel-ticket-action-dispatch`
- branch: `work/workspace-panel-ticket-action-dispatch`

This ticket may read tracked `.yoi/tickets` records/design artifacts. Do not read or edit `.yoi/memory/`.

## Requirements

- Replace blanket display-only Ticket action behavior with an action dispatcher for selected Ticket rows.
- Re-check current Ticket authority before mutation; do not mutate based only on stale `WorkspacePanelViewModel` data.
- Use Rust Ticket backend APIs / existing Ticket tool logic; do not shell out.
- Implement `Go` at minimum:
  - record a Ticket decision/comment indicating human Go for Orchestrator routing/preflight;
  - notify the workspace Orchestrator when live/reachable, if an existing peer/client path is available;
  - report notification failure as bounded warning, not lost Ticket decision.
- Implement `Defer` where existing Ticket backend semantics make it safe, likely by moving/open->pending or recording a pending/defer decision.
- `Close` must not close without a resolution; either provide a safe explicit resolution path or show a clear diagnostic that close requires a resolution.
- `Review` should not silently approve; either route to existing review command/role flow or show a clear diagnostic guiding the user to inspect evidence/use review path.
- No-Ticket workspaces must expose no Ticket actions and preserve Pod-centric behavior.
- Preserve existing selected-Pod open/direct-send and composer target behavior.
- Keep UI layout changes minimal; final visual tuning comes later.
- Do not reintroduce `--multi`.
- Do not add scheduler/queue/automatic coder/reviewer spawning.

## Suggested action semantics

- `Go`: append a `decision` or equivalent Ticket thread entry such as `Panel Go: user authorized Orchestrator routing/preflight`, then attempt to notify Orchestrator with a concise message naming the Ticket id/slug and action. Do not start implementation directly.
- `Defer`: if backend status `pending` is the established defer state, move Ticket to pending and append a decision/comment explaining panel defer; otherwise append a decision only and leave status unchanged with a diagnostic.
- `Close`: require explicit resolution text; for this first slice, a disabled diagnostic is acceptable.
- `Review`: disabled/guided diagnostic is acceptable unless there is already a safe review UI path.

## Current code map

- `crates/tui/src/multi_pod.rs`
  - Current row selection, Enter/send/open behavior, composer targets, actionbar notices.
- `crates/tui/src/workspace_panel.rs`
  - `PanelRow`, `PanelRowKey`, `TicketPanelEntry`, `NextUserAction`, row derivation. Add stable action keys/data if needed.
- `crates/ticket/src/lib.rs`
  - Ticket backend status/comment/review/close APIs.
- `crates/ticket/src/tool.rs`
  - Existing tool behavior for Ticket mutations may be reusable/referenceable.
- `crates/client/src/ticket_role.rs`
  - Existing Orchestrator/Intake launch context and role naming.
- Pod peer/client send helpers used by handoff/composer implementation.

## Validation

Run at least:

- targeted TUI tests for `Go` action success;
- targeted TUI tests for stale/invalid Ticket action rejection;
- targeted TUI tests for `Defer` behavior or explicit disabled reason;
- targeted tests that `Close` requires resolution / does not close silently;
- tests that no-Ticket workspace has no Ticket action dispatch;
- tests that Pod open/direct-send and composer Intake paths still work;
- `cargo test -p tui workspace_panel`;
- `cargo test -p tui multi_pod`;
- `cargo test -p ticket` if backend APIs change;
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
- exact action semantics implemented for Go/Defer/Review/Close;
- how Ticket authority is re-checked;
- how Orchestrator notification is attempted/reported;
- proof no automatic implementation scheduling is introduced;
- tests updated/added;
- validation results;
- remaining layout/display tuning items.
