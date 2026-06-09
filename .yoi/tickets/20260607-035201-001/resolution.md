Implemented, reviewed, merged, and validated.

Summary:
- Added role-specific Orchestrator/Coder/Reviewer guidance for accepted in-progress Tickets.
- Orchestrator guidance now routes only after `workflow_state = inprogress`, references `worktree-workflow` and `multi-agent-workflow`, records bounded progress, and stops at a merge-ready dossier.
- Coder guidance keeps implementation in the child worktree/branch, forbids main `.yoi` / Ticket / workflow / docs / memory edits, requires validation/reporting, and forbids merge/close/delete/push.
- Reviewer guidance keeps review sibling/read-only by default, focused on diff/intent/validation/blocker classification, with branch-local verdicts held for the merge-ready dossier rather than final main Ticket approval.
- Panel Queue notification guidance now describes the post-acceptance worktree/coder/reviewer sibling route and merge-ready dossier stop without implementing merge/close.
- No broad scheduler or worktree manager was added.

Implementation:
- Child commit: `c7d6bb8 orchestrator: add agent routing guidance`
- Merge commit: `merge: orchestrator agent routing`

Review:
- External reviewer `orchestrator-agent-routing-reviewer-20260607` approved with no blockers.

Validation after merge:
- `cargo test -p client ticket_role --lib`
- `cargo test -p tui ticket_queue_notification_message_carries_routing_contract --lib`
- `cargo fmt --check`
- `git diff --check`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`