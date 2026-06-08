<!-- event: create author: LocalTicketBackend at: 2026-06-08T03:22:00Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-08T03:31:04Z -->

## Intake summary

Existing Ticket is sufficiently specified for Orchestrator routing. It requests implementing the workspace Panel Close action only for safe already-done Tickets, using the existing Ticket backend close operation with deterministic non-LLM resolution text, bounded diagnostics for unsafe attempts, and Panel refresh after close. Acceptance criteria and validation are present; implementation must not close non-done Tickets, manually move files, launch workers for resolution text, or auto-commit the close behavior.

---

<!-- event: state_changed author: ticket-intake at: 2026-06-08T03:31:04Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake classified this Ticket as ready to queue: requirements, invariants, acceptance criteria, and validation are clear enough for Orchestrator routing.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T05:46:20Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T05:50:06Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Intake classified this Ticket as ready; requirements, safety conditions, deterministic resolution text, and acceptance criteria are explicit.
- The work is local to TUI/Panel Ticket action dispatch and existing Ticket backend close behavior; it does not require a new authority model or worker/LLM involvement.
- Remaining uncertainty is bounded implementation: exact helper placement, diagnostic wording, and test setup.

Evidence checked:
- Ticket body: background, goal, requirements, design notes, acceptance criteria.
- Thread: intake summary, `ready -> queued` event.
- Workspace state: no matching branch/worktree exists; active `split-direct-and-delegation-authority` worktree is separate and touches scope/runtime authority rather than Panel close dispatch.
- Code map search for `NextUserAction::Close`, current no-op close diagnostic, Ticket action dispatch, workspace panel safe-action derivation, and `LocalTicketBackend::close` / `TicketBackend::close`.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

IntentPacket:

Intent:
- Implement workspace Panel `Close` action for safe already-done open Tickets by calling the existing Ticket backend close operation with deterministic non-LLM resolution text.

Binding decisions / invariants:
- Only auto-close Tickets that are safe to close: local status is open, `workflow_state == done`, no blocking `attention_required`, no blocking `action_required`, and no existing `resolution.md`.
- Do not close Tickets whose workflow state is not `done`.
- Do not launch Orchestrator/Companion/worker solely to generate resolution text.
- Do not manually move Ticket files; use the existing typed Ticket backend close path.
- Do not auto-commit the close action behavior.
- Unsafe close attempts must produce bounded diagnostics and not mutate the Ticket.
- Panel must refresh after successful close so open rows reflect the moved Ticket.

Requirements / acceptance criteria:
- `NextUserAction::Close` dispatch performs the safe-close check and invokes the backend close operation with deterministic generated resolution.
- Generated resolution is concise/factual and notes that the Ticket had already reached `workflow_state: done`, with no implementation/workflow-state changes started by the close action.
- Closed Ticket has `resolution.md`; backend thread/status close behavior is preserved.
- Blocked close attempts explain the blocker.
- Tests cover successful close, blocked close, generated resolution content, and non-done Tickets not closing.

Implementation latitude:
- Coder may choose helper names and whether safe-close validation lives near action dispatch or workspace panel row derivation.
- Coder may refine exact deterministic resolution wording while preserving the facts requested by the Ticket.
- Coder may add focused backend/TUI tests around `dispatch_ticket_action` rather than E2E keypress tests.

Escalate if:
- Existing backend close API cannot be called without a broader TUI/backend dependency change.
- `action_required` semantics are ambiguous enough that a human decision is needed; default to blocking when in doubt.
- Closing from Panel would require committing or invoking a worker.

Validation:
- Focused TUI tests around `dispatch_ticket_action` / `NextUserAction::Close`.
- Existing workspace panel action derivation tests.
- `cargo test -p tui workspace_panel --lib` and/or `cargo test -p tui multi_pod --lib` focused equivalents selected by coder.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- Because TUI/Ticket behavior is touched, final merge-completion should include `nix build .#yoi`.

Current code map:
- `crates/tui/src/workspace_panel.rs`: `NextUserAction::Close` derivation and safe/action metadata.
- `crates/tui/src/multi_pod.rs`: `prepare_ticket_action_dispatch`, `dispatch_ticket_action`, current Close no-op, Ticket action tests.
- `crates/ticket/src/lib.rs`: `TicketBackend::close`, `LocalTicketBackend::close`, resolution handling and close tests.
- `crates/yoi/src/ticket_cli.rs` / ticket tools only as examples of invoking backend close with resolution.

Critical risks / reviewer focus:
- Close must not run for non-done or blocked Tickets.
- Close must use backend close, not manual file moves.
- Resolution text must be deterministic and not LLM-generated.
- Panel refresh/notice behavior must make the close visible without requiring manual reload.
- Tests should assert no mutation on unsafe attempts.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T05:50:11Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after reading the Ticket, workspace state, and Panel/Ticket close code map. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---
