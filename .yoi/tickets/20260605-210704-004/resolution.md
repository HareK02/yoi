Implemented first workspace panel composer targets.

Changes:
- Added workspace panel composer target model with:
  - `Companion` / existing selected-Pod send behavior;
  - `Ticket Intake`.
- Added `Ctrl+T` target switching without clearing typed composer text.
- Displayed active target in the panel target/status line and actionbar guidance using existing TUI conventions.
- No-Ticket and unusable-Ticket-config workspaces expose only `Companion` / Pod-centric behavior.
- `Ticket Intake` is available only when Ticket config is usable.
- Empty Intake input is rejected with a bounded diagnostic.
- Intake launch uses the existing `TicketRole::Intake` / `launch_ticket_role_pod(...)` path.
- Composer text is passed as `TicketRoleLaunchContext.user_instruction` for the Intake launch.
- Intake text is not sent to Companion/current Pod history or selected-Pod direct send path.
- Successful Intake launch clears composer and reports the launched Pod name.
- Launch failure keeps composer text and reports a bounded diagnostic.
- Intake -> Orchestrator handoff payload was intentionally not added; the follow-up handoff ticket owns that.
- `--multi` was not reintroduced.

Validation after merge:
- `cargo test -p tui workspace_panel`
- `cargo test -p tui multi_pod`
- `cargo test -p client ticket_role`
- `cargo test -p yoi panel`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

External review approved with no requested changes.

Known follow-up:
- Active target display is intentionally minimal; final placement/color/wording tuning is deferred.
- Intake -> Orchestrator handoff remains the next child ticket.
