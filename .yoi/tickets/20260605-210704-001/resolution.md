Implemented Ticket Intake -> Orchestrator handoff for the workspace panel.

Changes:
- Added typed handoff contract `TicketIntakeHandoff { orchestrator_pod, workspace_label }`.
- Extended `TicketRoleLaunchContext` with optional Intake handoff data.
- Rendered a `Panel handoff:` section into Intake launch input/history through the existing Ticket role launcher.
- The handoff section includes:
  - workspace label;
  - workspace Orchestrator Pod name;
  - expected readiness/report fields;
  - an explicit no-auto-implementation gate requiring Orchestrator routing/preflight and human Go before implementation.
- Added `TicketRoleLaunchOptions` / `launch_ticket_role_pod_with_options(...)` for bounded pre-run peer registration.
- Preserved `launch_ticket_role_pod(...)` as the default no-options wrapper.
- Panel Intake launch now passes Orchestrator handoff data and, when applicable, a pre-run peer registration request.
- `RegisterPeer` is sent before the first `Method::Run`, avoiding the controller's running-state rejection path.
- Peer registration warnings are non-fatal when Intake launch/run succeeds.
- No-Ticket workspaces remain Companion-only and expose no handoff path.
- Intake requests do not route through Companion/current Pod history or selected-Pod direct send.
- No automatic implementation scheduling/coder/reviewer spawning was added.
- `--multi` was not reintroduced.

Validation after merge:
- `cargo test -p client ticket_role`
- `cargo test -p tui workspace_panel`
- `cargo test -p tui multi_pod`
- `cargo test -p yoi panel`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

External review approved after one requested-changes cycle.

Known follow-up:
- Final layout/display wording for handoff notices is deferred to panel tuning.
