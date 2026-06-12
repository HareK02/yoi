Panel Queue handoff implementation was merged from `ticket/panel-queue-orchestrator-sync` into `orchestration/yoi-orchestrator`, then merged into `develop`.

Implemented behavior:
- Panel Queue performs the root-side Ticket `ready -> queued` mutation and Queue commit.
- The dedicated orchestration worktree is synchronized with ff-only semantics before Orchestrator notification.
- Queue success/failure paths report Ticket id, Queue commit, sync result, and notification outcome.
- Dirty/divergent/state-mismatch/worktree-identity failure cases are blocked with explicit diagnostics.

Validation after merge:
- `cargo fmt`
- `cargo test -p client ticket_role`
- `cargo test -p tui ticket_queue_notification_message_carries_routing_contract`
- `target/debug/yoi ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`
- `cargo test -p tui ticket_queue_notification_message_carries_routing_contract`

Cleanup:
- Removed implementation worktree `/home/hare/Projects/yoi/.worktree/panel-queue-orchestrator-sync`.
- Deleted merged branch `ticket/panel-queue-orchestrator-sync`.
