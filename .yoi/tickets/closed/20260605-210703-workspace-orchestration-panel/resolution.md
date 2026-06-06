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
