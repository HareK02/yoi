<!-- event: create author: yoi ticket at: 2026-06-05T21:07:04Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-06T04:49:16Z -->

## Plan

Preflight result: `implementation-ready` after Orchestrator lifecycle and composer target slices.

This ticket should connect panel-launched Intake Pods to the workspace Orchestrator with an explicit, auditable handoff target. The handoff should be carried through the existing Ticket role launch path, preferably as a typed launch-context field rendered into the Intake first-run input/history, and peer metadata should be registered when feasible. It must not authorize automatic implementation.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-06T05:27:40Z status: approve -->

## Review: approve

External reviewer approved current HEAD after requested changes were addressed.

Review summary:
- The previous blocking issue was fixed: `RegisterPeer` is now sent through the Ticket role launcher before the first `Method::Run`.
- Handoff uses a small typed `TicketIntakeHandoff` contract and `TicketRoleLaunchOptions`, not scheduler state.
- Intake launch input includes Orchestrator target and the no-auto-implementation gate.
- No-Ticket workspaces remain Companion-only/no handoff.
- Peer registration failures/skips are bounded warnings, not launch failures.
- Intake request does not route through Companion/current Pod history or selected-Pod direct send.
- No automatic implementation scheduling or `--multi` reintroduction.


---

<!-- event: close author: hare at: 2026-06-06T05:27:40Z status: closed -->

## Closed

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


---
