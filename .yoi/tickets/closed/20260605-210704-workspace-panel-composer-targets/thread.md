<!-- event: create author: yoi ticket at: 2026-06-05T21:07:04Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-06T04:24:53Z -->

## Plan

Preflight result: `implementation-ready` after action model and Orchestrator lifecycle.

This ticket should add the first composer target split for `yoi panel`: preserve the Companion/selected-Pod send path, and add a Ticket Intake target that launches an Intake role Pod with the composer body as the initial user instruction. The Intake body must not be appended to Companion/current Pod history or sent to the selected Pod.

Ticket Intake target is available only when Ticket config is defined/usable. No-Ticket workspaces remain Pod-centric and expose only the existing send behavior.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-06T04:47:31Z status: approve -->

## Review: approve

External reviewer approved commit `a7c155e` with no blocking issues.

Review summary:
- Composer targets include `Companion` and `Ticket Intake`.
- No-Ticket/unusable-Ticket-config workspaces remain Companion-only.
- Active target is visible and `Ctrl+T` switching preserves typed input.
- Ticket Intake uses `TicketRole::Intake` via `launch_ticket_role_pod` with composer text as `TicketRoleLaunchContext.user_instruction`.
- Intake text does not route through the selected-Pod direct send path.
- Empty Intake input is rejected locally with a bounded diagnostic.
- Success clears composer and reports launched Pod name; failure keeps composer and reports a bounded diagnostic.
- No Intake -> Orchestrator handoff payload was added.
- `--multi` was not reintroduced.


---

<!-- event: close author: hare at: 2026-06-06T04:47:31Z status: closed -->

## Closed

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


---
