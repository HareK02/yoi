# Delegation intent: workspace panel orchestrator lifecycle

## Classification

`implementation-ready` after the first action-model slice.

The action model slice added `yoi panel`, removed `--multi` as a user-facing launch route, and established no-Ticket behavior: if workspace Ticket config is absent, the panel suppresses Ticket UI and behaves as a Pod-centric dashboard. This lifecycle ticket should respect that boundary.

## Intent

When `yoi panel` opens in a Ticket-enabled workspace, ensure a workspace Orchestrator Pod exists or surface a bounded diagnostic explaining why it cannot be restored/spawned. The Orchestrator remains a background coordinator; closing the panel must not stop it, and Companion remains the foreground management target.

In a no-Ticket workspace, do not spawn/restore an Orchestrator and do not show Orchestrator/Ticket workflow affordances. The panel should remain equivalent to the old Pod-centric `--multi` behavior.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/workspace-panel-orchestrator-lifecycle`
- branch: `work/workspace-panel-orchestrator-lifecycle`

This ticket may read tracked `.yoi/tickets` records/design artifacts. Do not read or edit `.yoi/memory/`.

## Requirements

- Derive the workspace Orchestrator Pod name from the workspace directory, e.g. `<dir-name>-orchestrator`.
  - Use a safe/stable Pod-name normalization compatible with existing Pod name constraints.
  - Add tests for common workspace names and edge cases.
- On `yoi panel` open in a Ticket-enabled workspace:
  - if Orchestrator is already live, observe/report it as live;
  - if restorable, restore it through existing Pod restore semantics;
  - if missing and permitted, spawn it;
  - if restore/spawn is not possible, surface a bounded panel diagnostic.
- Gate Orchestrator lifecycle on explicit Ticket config availability/usability.
  - If `.yoi/ticket.config.toml` is absent, skip Orchestrator lifecycle and show Pod-centric panel only.
  - If config exists but is malformed/unusable, surface a diagnostic rather than silently behaving as no-Ticket if practical in this slice.
- Use existing Ticket role/profile configuration and role launcher where practical; do not hand-build role prompts/profiles inside TUI.
- Preserve current Pod registry/restore/spawn semantics; do not duplicate registry logic.
- Panel close must not stop the Orchestrator.
- Do not make Orchestrator the foreground composer target by default.
- Keep layout changes minimal; final display tuning is later.

## Non-goals

- Composer target switching / New Intake send.
- Intake -> Orchestrator handoff payload.
- Scheduler/lease/queue.
- Automatic coder/reviewer spawning.
- Final layout/display tuning.
- Reintroducing `--multi`.

## Current code map

- `crates/tui/src/lib.rs`
  - `LaunchMode::Panel` path.
- `crates/tui/src/single_pod.rs`
  - `run_panel(...)` loop around the panel/dashboard and nested Pod open behavior.
- `crates/tui/src/multi_pod.rs`
  - current panel substrate, snapshot loading, rows/rendering/actions.
- `crates/tui/src/workspace_panel.rs`
  - thin panel ViewModel / Ticket row model. Extend header/diagnostics as needed without making it a runtime authority.
- `crates/client/src/ticket_role.rs`
  - Ticket role launcher path; prefer this for spawning role Pods if it fits panel lifecycle.
- `crates/ticket/src/config.rs`
  - Ticket config detection/loading; use this to gate Ticket-enabled vs no-Ticket mode.
- Existing Pod restore/list/spawn client APIs used by picker/multi/role launcher.

## Validation

Run at least:

- targeted TUI tests for name derivation and lifecycle decision logic;
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
- final Orchestrator Pod naming rule;
- lifecycle decision behavior for live/restorable/missing/unavailable;
- no-Ticket workspace behavior;
- role launcher/profile usage;
- diagnostics surfaced;
- tests updated/added;
- validation results;
- known follow-up layout/display tuning items.
