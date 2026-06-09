Merged and completed the workflow-state planning replacement.

Summary:
- Replaced user-facing/model-output `TicketWorkflowState::Intake` with `Planning`.
- Legacy `workflow_state: intake` parses compatibly as `planning`; emitted/default state is `planning`.
- Updated transition graph to `planning -> ready -> queued -> inprogress -> done`.
- Added audited return-to-planning transitions from `ready` and `queued` with reason/body through typed state-change events.
- Preserved Intake role/profile/persona/tool names where they refer to role identity, e.g. `TicketIntakeReady`.
- Removed new `needs_preflight` tool input exposure; legacy metadata remains only for read/compatibility and is no longer a stop gate.
- Updated Panel labels/actions and planning/clarification/preparation terminology.
- Reworked active workflows/docs to fold old preflight language into planning/requirements sync. `ticket-preflight-workflow` remains only as compatibility slug with planning/requirements-sync content.

Merged branch/worktree:
- Branch: `replace-intake-state-with-planning`
- Commits: `ada6db9`, `eddb33c`
- Merge commit on `develop`: `64466f2 merge: replace intake state with planning`

Validation passed after merge:
- `cargo fmt --check`
- `git diff --check`
- `cargo test -p ticket --lib`
- `cargo test -p ticket workflow_state --lib`
- `cargo test -p ticket ticket_workflow_tool --lib`
- `cargo test -p ticket ticket_intake_ready_tool --lib`
- `cargo test -p tui workspace_panel --lib`
- `cargo test -p yoi ticket_cli`
- `cargo check --workspace`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/replace-intake-state-with-planning`.
- Deleted branch `replace-intake-state-with-planning`.

Residual notes:
- Historical `.yoi/tickets/**` records still contain old `Preflight`, `preflight_needed`, and `needs_preflight` text as durable audit/history artifacts; they were intentionally not rewritten.
- `ticket-preflight-workflow` filename remains as a compatibility slug, but its content now explicitly describes planning/requirements sync rather than a separate workflow state/lane/long-lived operation.