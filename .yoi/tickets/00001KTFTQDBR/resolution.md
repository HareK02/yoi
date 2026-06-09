Implemented, externally reviewed, merged, and validated.

Summary:
- Added a local workspace-scoped panel role session registry under user data.
- Added local Ticket claim files to enforce at most one active local Pod claim per Ticket.
- Supported pre-Ticket Intake sessions and 0..N related Tickets per role session.
- Preserved `ticket-*` Pod heuristic visibility while adding local claim status display.
- Existing-Ticket Intake now claims before launch and refuses to silently start a second Pod when a live/restorable/stale claim exists.
- Local Pod assignment is not written into git-tracked Ticket metadata/thread.
- Preserved the already-merged no-direct-selected-Pod-send behavior.

Merged implementation:
- `4890590 tui: add panel role session registry`
- `2f3f54b fixup! tui: add panel role session registry`
- Merge commit: `merge: panel role session registry`

Review:
- Initial review requested changes for direct-send semantic conflict, incomplete registry schema, and launch failure/recording behavior.
- Fixup addressed blockers.
- Follow-up review approved with only non-blocking follow-ups: stale-lock recovery and temp claim file housekeeping.

Validation after merge:
- `cargo test -p tui role_session_registry --lib`
- `cargo test -p tui workspace_panel --lib`
- `cargo test -p tui multi_pod --lib`
- `cargo fmt --check`
- `git diff --check`
- `target/debug/yoi ticket doctor`