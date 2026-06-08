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

<!-- event: plan author: orchestrator at: 2026-06-08T05:50:36Z -->

## Plan

Implementation routing accepted and worktree created.

Worktree plan:
- Branch: `panel-close-done-tickets`
- Worktree: `.worktree/panel-close-done-tickets`
- Base: `c6e8f6a ticket: route panel close done tickets`
- Scope: implement safe workspace Panel close action for already-done Tickets using the typed Ticket backend.

Delegation plan:
- Spawn sibling coder Pod with write scope limited to `.worktree/panel-close-done-tickets`.
- Coder must run Bash commands from the child worktree, not from the main workspace.
- Coder must not edit main-workspace `.yoi` / Ticket / workflow / docs records, and must not create generated memory/local/runtime/secret-like files.
- Reviewer will be spawned read-only after coder reports diff/commits and validation evidence.

Concurrency note:
- `split-direct-and-delegation-authority` is active in a separate worktree. This Panel close implementation must remain isolated and should not edit that worktree or depend on unmerged authority-split changes.

Stop/completion policy:
- Because the human authorized cleanup after completion, this Orchestrator will proceed through reviewer approval, merge-completion validation, Ticket close, Pod stop, worktree removal, and branch deletion if the branch is approved and merge authority remains clear.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T06:03:26Z -->

## Implementation report

Coder completed and reviewer delegated.

Coder result:
- Pod: `coder-panel-close-done`
- Commit: `6d41ed3 tui: close done tickets from panel`
- Worktree status: clean after commit.
- Diff against `develop`: 1 file changed, 207 insertions, 77 deletions.

Files touched by coder:
- `crates/tui/src/multi_pod.rs`

Coder reported implementation:
- Added Panel Close dispatch using `LocalTicketBackend::close` for safe open/done Tickets only.
- Safe conditions checked: open status, `workflow_state == done`, empty/unset `attention_required`, empty/unset `action_required`, and no existing `resolution.md`.
- Generated deterministic resolution text noting that the Ticket had already reached `workflow_state: done`, and that the close action did not start implementation/workflow-state changes or launch Orchestrator/Companion/workers.
- Unsafe close attempts do not mutate and return bounded diagnostics.
- Added focused tests for success, generated resolution content, non-done blocking, `action_required` blocking, `attention_required` blocking, and existing resolution blocking.

Coder reported validation:
- `cargo test -p tui multi_pod --lib`
- `cargo test -p tui workspace_panel --lib`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Reviewer delegation:
- Spawned sibling reviewer Pod: `reviewer-panel-close-done`.
- Reviewer scope: read-only child worktree plus non-recursive parent-root read required by launch cwd validation.
- Reviewer was instructed to judge against the recorded Ticket requirements and binding decisions.

Pending:
- Await reviewer verdict before merge-ready dossier / merge-completion.
- No merge, close, final approval, or cleanup has occurred for this Ticket.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T06:09:31Z -->

## Implementation report

Merge-ready dossier: panel close done Tickets

Ticket id/slug:
- `20260608-032200-panel-close-done-tickets` / `panel-close-done-tickets`

Branch/worktree:
- Branch: `panel-close-done-tickets`
- Worktree: `.worktree/panel-close-done-tickets`
- Current branch commit:
  - `6d41ed3 tui: close done tickets from panel`

Intent / invariant check:
- Panel `Close` now closes only safe open Tickets already at `workflow_state: done`.
- The close action uses `LocalTicketBackend::close(...)`; it does not manually move files.
- Resolution text is deterministic/non-LLM and records that the Ticket was already done and no implementation/workflow-state/worker action was started by the close.
- Unsafe attempts return bounded diagnostics before mutation.
- Panel refresh is triggered after the action path so closed rows disappear from open list.

Implementation summary:
- Added `NextUserAction::Close` dispatch handling in `crates/tui/src/multi_pod.rs`.
- Added backend reload/safe-close guard for local status open, `workflow_state == done`, no non-empty `attention_required`, no non-empty `action_required`, and no existing `resolution.md`.
- Added deterministic resolution builder.
- Added focused tests for success, generated resolution content, non-done block, `action_required` block, `attention_required` block, and existing resolution block.

Files touched:
- `crates/tui/src/multi_pod.rs`

Coder / reviewer Pods:
- Coder: `coder-panel-close-done`
- Reviewer: `reviewer-panel-close-done`

Review evidence:
- Reviewer verdict: `approve`.
- Reviewer confirmed backend close is used, safe-close checks match Ticket requirements, unsafe cases return before mutation, deterministic resolution text is non-LLM, dispatch diagnostics are bounded, and reload is scheduled after dispatch.
- Reviewer noted non-open status has implementation guard but no dedicated test; judged non-blocking because other unsafe cases and success path are covered.

Validation performed by coder and/or reviewer:
- `cargo test -p tui multi_pod --lib`
- `cargo test -p tui workspace_panel --lib`
- `git diff --check develop...HEAD`
- `cargo fmt --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Blockers fixed or rejected findings:
- No reviewer blockers.

Residual risks:
- Non-open local status blocker lacks a dedicated focused test, though implementation explicitly checks `ticket.meta.status.as_local() == Some(TicketStatus::Open)` before closing.

Dirty state:
- Child worktree is clean at `6d41ed3`.
- Main workspace has unrelated Ticket-record edits and an active `split-direct-and-delegation-authority` worktree; they are outside this branch's touched paths and are understood.

Parent/human decision needs:
- User has authorized merge-completion and cleanup after approved work. Proceeding to merge-completion unless post-merge validation fails.

---

<!-- event: review author: orchestrator at: 2026-06-08T06:10:45Z status: approve -->

## Review: approve

Final merge-completion approval after merge to `develop` and post-merge validation.

Evidence:
- Merged branch `panel-close-done-tickets` with `--no-ff`.
- Reviewer `reviewer-panel-close-done` approved the branch-local implementation.
- Post-merge validation passed: `cargo test -p tui multi_pod --lib`, `cargo test -p tui workspace_panel --lib`, `cargo check -q`, `cargo fmt --check`, `git diff --check`, `cargo run -q -p yoi -- ticket doctor`, and `nix build .#yoi`.
- Coder/reviewer Pods stopped and delegated scope reclaimed.
- Merged worktree removed and branch deleted.

This approval is for the merged main-branch result, not merely the branch-local reviewer verdict.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T06:10:45Z from: inprogress to: done reason: merged_and_validated field: workflow_state -->

## State changed

Merged to `develop`, post-merge validation passed, final merge-completion approval recorded, and panel-close branch/worktree/Pods cleaned up.

---

<!-- event: close author: hare at: 2026-06-08T06:10:59Z status: closed -->

## Closed

Merged and completed the workspace Panel close-done-Tickets action.

Summary:
- Panel `Close` action now closes safe already-done open Tickets through `LocalTicketBackend::close(...)`.
- Safe-close guard requires local status open, `workflow_state: done`, no non-empty `attention_required`, no non-empty `action_required`, and no existing `resolution.md`.
- Unsafe close attempts return bounded diagnostics before mutation.
- Generated resolution text is deterministic/non-LLM and states that the Ticket was already done and the close action did not start implementation, workflow-state changes, Orchestrator/Companion launch, or worker invocation.
- Panel refresh is scheduled after the action path so closed rows leave the open list.

Merged branch/worktree:
- Branch: `panel-close-done-tickets`
- Commit: `6d41ed3 tui: close done tickets from panel`
- Merge commit on `develop`: `2415956 merge: close done tickets from panel`

Validation passed after merge:
- `cargo test -p tui multi_pod --lib`
- `cargo test -p tui workspace_panel --lib`
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/panel-close-done-tickets`.
- Deleted branch `panel-close-done-tickets`.

Residual note:
- Non-open local status blocker has an explicit implementation guard but no dedicated focused test; reviewer accepted this as non-blocking because success and other unsafe cases are covered.

---
