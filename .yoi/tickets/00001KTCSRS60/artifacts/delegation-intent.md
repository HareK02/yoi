# Delegation intent: Ticket Intake -> Orchestrator handoff

## Classification

`implementation-ready` after Orchestrator lifecycle and composer target slices.

`yoi panel` now opens the workspace panel, ensures a workspace Orchestrator for Ticket-enabled workspaces, and can launch a Ticket Intake Pod from the composer. This ticket connects those pieces with an explicit handoff contract.

## Intent

When the panel launches a Ticket Intake Pod, provide the workspace Orchestrator as the Intake's notification/handoff target so the Intake can report clarified Ticket readiness to the Orchestrator. The handoff must be visible in the destination Pod's committed input/history or durable peer metadata, not hidden context.

The handoff should not authorize implementation automatically. It only tells Intake where to report readiness or follow-up state after Intake has clarified and materialized/agreed a Ticket.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/ticket-intake-orchestrator-handoff`
- branch: `work/ticket-intake-orchestrator-handoff`

This ticket may read tracked `.yoi/tickets` records/design artifacts. Do not read or edit `.yoi/memory/`.

## Requirements

- Define a simple handoff contract from workspace panel to Intake.
  - Include Orchestrator Pod name.
  - Include that Intake should notify/report to the Orchestrator when it has enough information for routing/preflight.
  - State clearly that implementation must not start automatically; human Go / Orchestrator routing gates still apply.
- Carry the handoff through the existing Ticket role launch path where practical.
  - Prefer adding explicit fields/options to `TicketRoleLaunchContext` or equivalent over ad hoc string assembly in TUI.
  - Do not duplicate role prompt/profile construction in TUI.
- Register Intake and Orchestrator as peers when both are live/reachable if existing `RegisterPeer` semantics make this feasible.
  - Registration failure should be a bounded diagnostic/warning, not a reason to lose the Intake launch if the launch itself succeeded.
  - Do not rely only on an LLM-internal instruction if a durable peer metadata path is available.
- Include the handoff instruction in the Intake launch input/history so the Intake can explain why it may notify the Orchestrator in later turns.
  - Do not inject hidden context only.
  - Do not append the Intake request to Companion/current Pod history.
- Add panel success/failure diagnostics for handoff setup separately from Intake launch success.
- No-Ticket workspace must not expose this path.
- Keep Orchestrator as background coordinator; do not make it foreground composer target by default.
- Keep the contract small and typed enough to test, but do not build a scheduler/queue.

## Suggested contract shape

A small data type is preferred over freeform string construction in the panel:

```rust
TicketIntakeHandoff {
    orchestrator_pod: String,
    workspace_label: String,
}
```

The role launcher can render this into the Intake `user_instruction` as an explicit section, for example:

```text
Panel handoff:
- Workspace Orchestrator Pod: <name>
- When Intake has clarified the request and created/updated the Ticket, notify/report readiness to this Orchestrator.
- Do not start implementation automatically; wait for Orchestrator routing/preflight and human Go gates.
```

If peer registration is implemented, Intake should also receive durable peer metadata for the Orchestrator using the existing Pod peer registration path.

## Non-goals

- Automatic implementation scheduling.
- Queue/lease system.
- Full Ticket action model refinement.
- Final layout/display tuning.
- Generic arbitrary handoff routing.
- Reintroducing `--multi`.

## Current code map

- `crates/tui/src/multi_pod.rs`
  - Panel composer target handling and Intake launch path.
- `crates/tui/src/workspace_panel.rs`
  - Orchestrator state/name and composer target view state.
- `crates/client/src/ticket_role.rs`
  - Role launch context and prompt/input assembly; preferred place for typed handoff fields.
- `crates/protocol/src/lib.rs`
  - `Method::RegisterPeer` / `Event::PeerRegistered` semantics.
- `crates/pod/src/controller.rs`
  - Peer registration handling for reference.
- `crates/tui/src/socket.rs` / existing client helpers
  - Reuse existing socket/method helpers where practical; do not hand-roll protocol loops unnecessarily.

## Validation

Run at least:

- targeted tests for role launch input rendering with handoff;
- targeted tests for panel Intake launch including handoff data;
- targeted tests for peer registration success/failure behavior if implemented;
- `cargo test -p client ticket_role`;
- `cargo test -p tui workspace_panel`;
- `cargo test -p tui multi_pod` or updated panel test names;
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
- final handoff contract/data type;
- how Orchestrator target is included in Intake input/history;
- whether/how peer registration is performed;
- launch vs handoff diagnostic behavior;
- proof no Companion/current Pod history mutation is used for Intake body;
- tests updated/added;
- validation results;
- known follow-up layout/display tuning items.
