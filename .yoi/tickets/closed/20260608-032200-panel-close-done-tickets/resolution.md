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