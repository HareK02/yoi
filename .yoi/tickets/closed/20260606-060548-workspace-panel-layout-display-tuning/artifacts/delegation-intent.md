# Delegation intent: workspace panel layout/display tuning

## Classification

`implementation-ready` as a focused UX/layout pass after the first-pass panel implementation.

The main requested change is row readability: Ticket titles and Pod names/ids are variable-length and currently lead the row, which makes status/action scanning hard. Rework rendering into aligned columns with short fields first and long free-text identifiers last.

## Intent

Tune `yoi panel` row rendering and immediate display affordances without changing Ticket backend/action semantics.

Primary direction:

- Ticket/action rows: short aligned columns first, long Ticket title last.
- Pod rows: short aligned columns first, variable-length Pod id/name last.
- Ticket row and Pod row schemas do not need to be identical, but each row family should align its own columns consistently.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/workspace-panel-layout-display-tuning`
- branch: `work/workspace-panel-layout-display-tuning`

This ticket may read tracked `.yoi/tickets` records/design artifacts. Do not read or edit `.yoi/memory/`.

## Requirements

- Rework row rendering into stable aligned columns rather than sentence-like rows.
- Ticket/action row layout should move the long Ticket title to the end.
  - Put short, comparable fields first.
  - Suggested fields/order: selection marker, priority bucket, action, derived status, phase, short id/slug, title.
  - Exact widths can be tuned, but status/action/phase should line up across Ticket rows.
- Pod row layout should move variable-length Pod name/id to the end.
  - Put short, comparable fields first.
  - Suggested fields/order: selection marker, status, action, role/kind if cheaply available, Pod name/id.
  - Long Pod names must not break status/action alignment.
- Preserve existing color/style semantics unless a small adjustment improves readability.
- Improve labels only where it directly helps the aligned layout; avoid broad copy rewrites.
- Preserve `yoi panel` behavior in both Ticket-enabled and no-Ticket workspaces.
- Preserve Ticket action dispatch, Intake launch, Orchestrator lifecycle, Pod open/direct-send, and composer target behavior.
- Do not move authority into rendering code; keep the thin ViewModel boundary.
- Do not reintroduce `--multi`.
- Avoid broad refactors or new scheduler/action semantics.

## Current code map

- `crates/tui/src/multi_pod.rs`
  - `panel_row_line(...)` currently renders Ticket/action rows as `<title> [priority] Action: status ...`.
  - `row_line(...)` currently renders Pod rows as `<pod name> [status] action ...`.
  - `panel_priority_style(...)`, `row_status_label(...)`, section/header rendering, and tests are nearby.
- `crates/tui/src/workspace_panel.rs`
  - `PanelRow`, `TicketPanelEntry`, `ActionPriority`, `NextUserAction`, `TicketPanelPhase`, and `ticket_subtitle(...)` provide the display fields.
  - Extend display-ready fields only if needed; keep this as UI data, not authority.
- Existing tests in `crates/tui/src/multi_pod.rs` and `workspace_panel.rs`
  - Add/adjust unit tests for row rendering/alignment and truncation behavior.

## Suggested display shape

Ticket/action row example:

```text
▶ decision  Review  implementation reported  review       workspace-panel-composer-targets  Workspace panel composer targets
  ready     Go      ready for Go              preflight    ticket-slug                       Long Ticket title...
```

Pod row example:

```text
  live idle      send   pod   companion
  live running   open   pod   very-long-background-worker-name...
```

Exact wording and widths can differ, but the visual rule is fixed: short comparable columns first, long names/titles last.

## Non-goals

- Backend/Ticket action semantic changes.
- New scheduler/queue behavior.
- Orchestrator/Intake protocol changes.
- Replacing the single-Pod TUI.
- Reintroducing `--multi`.
- Large module refactors.

## Validation

Run at least:

- targeted TUI tests for Ticket row rendering/alignment;
- targeted TUI tests for Pod row rendering/alignment;
- `cargo test -p tui workspace_panel`;
- `cargo test -p tui multi_pod`;
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
- final Ticket row column schema and widths/truncation behavior;
- final Pod row column schema and widths/truncation behavior;
- tests updated/added;
- validation results;
- remaining UX/display tuning items, if any.
