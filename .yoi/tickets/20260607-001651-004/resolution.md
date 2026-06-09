Implemented and merged.

Summary:
- Removed the workspace panel path that treated the `Companion` composer target as direct selected-Pod `Method::Run` sending.
- Non-empty Companion submit now preserves the draft and reports that the real Companion lifecycle is not connected yet.
- Preserved Pod open/attach behavior through empty Enter / `o`.
- Preserved Ticket Intake composer launch behavior.
- Updated Pod row action/key-hint behavior away from direct-send semantics.
- Updated focused tests for the new behavior.

Merged implementation:
- Child commit: `393cde9 tui: remove panel direct pod send`
- Main merge commit: `merge: remove panel direct pod send`

Validation after merge:
- `cargo test -p tui multi_pod --lib`
- `cargo test -p tui workspace_panel --lib`
- `cargo fmt --check`
- `git diff --check`