# Delegation intent: workspace panel action model

## Classification

`implementation-ready` as the first implementation slice after design approval.

The design ticket is closed and approved. The first pass should implement a simple, testable UI/action model that the existing multi-Pod dashboard can consume before we tune layout details.

## Intent

Add a thin workspace panel ViewModel/action model for Ticket-centric display and prioritization. This should be a render/action-dispatch contract, not a new Ticket backend, scheduler, or UI-owned state machine.

The panel should be local-file-first for display:

- read Ticket records from `.yoi/tickets/` through the Rust Ticket backend/config APIs;
- combine with existing Pod list data for related/background Pod status where practical;
- keep socket/client operations out of the model layer.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/workspace-panel-action-model`
- branch: `work/workspace-panel-action-model`

This ticket may edit tracked `.yoi/tickets` records for implementation reports if needed. Do not read or edit `.yoi/memory/`.

## Requirements

- Define plain UI/data model types for the workspace panel, for example:
  - `WorkspacePanelViewModel`;
  - `PanelRow` / row key;
  - `TicketPanelEntry`;
  - action priority / next user action;
  - optional timeline/dependency lane structs if useful for the first slice.
- Keep the model independent from terminal rendering and from live socket I/O.
- Add an adapter that builds Ticket/action rows from local Ticket backend state.
- Reuse existing `PodList`/multi-Pod data as lower-priority background rows where practical, without replacing or duplicating Pod registry logic.
- Prioritize rows roughly as:
  1. Intake/user reply/approval needed;
  2. Ticket ready for Go;
  3. review/close decision required;
  4. blocked/action-required;
  5. active implementation/review/background work;
  6. passive Pod/session information.
- Use simple heuristics from current Ticket records/thread roles; do not invent a full scheduler or hidden state machine.
- Preserve human authorization boundaries: a displayed Go/Review/Close action is an affordance, not automatic implementation authorization.
- Integrate enough with the current `--multi` dashboard to display action-prioritized Ticket rows above passive Pod rows, while keeping current Pod attach/direct-send behavior intact as much as possible.
- Follow existing TUI visual conventions; do not spend this ticket on fine layout tuning.

## Non-goals

- Final layout polish or detailed display tuning.
- Orchestrator restore/spawn lifecycle implementation.
- Composer target switching / New Intake send path.
- Intake -> Orchestrator handoff contract.
- Scheduler/lease/queue automation.
- Replacing the normal single-Pod TUI.

## Current code map

- `crates/tui/src/multi_pod.rs`
  - Current `--multi` dashboard state, reload, sections, rendering, direct send/open behavior.
  - Integrate only enough to show Ticket/action rows and preserve existing behavior.
- `crates/tui/src/pod_list.rs`
  - Existing plain Pod list model. Reuse rather than duplicating live/stored Pod discovery semantics.
- `crates/tui/src/lib.rs` / `crates/tui/src/single_pod.rs`
  - Current `LaunchMode::Multi` path enters `single_pod::run_multi(...)`.
- `crates/ticket/src/lib.rs`, `crates/ticket/src/config.rs`
  - Local Ticket backend/config API for reading `.yoi/tickets/`.
- `crates/yoi/src/ticket_cli.rs`
  - Examples of backend listing/status formatting; do not shell out.
- Design artifact:
  - `.yoi/tickets/closed/20260605-210704-workspace-orchestration-panel-design/artifacts/workspace-panel-ui-design.md`

## Validation

Run at least:

- `cargo test -p tui multi` or targeted TUI tests covering the new model/render integration;
- `cargo test -p ticket` if Ticket backend API usage changes;
- `cargo test -p yoi ticket` if CLI/config interactions change;
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
- model/types added;
- how Ticket rows are derived and prioritized;
- how Pod rows are preserved as secondary/background information;
- tests updated/added;
- validation results;
- known follow-up layout/display tuning items.
