Implemented and merged.

Summary:
- Removed `investigator` from the fixed Ticket role set; remaining fixed roles are `intake`, `orchestrator`, `coder`, and `reviewer`.
- Updated Ticket config parsing and scaffold tests so `[roles.investigator]` is not emitted and stale `[roles.investigator]` config is rejected with a clear unsupported-role diagnostic.
- Updated role docs/workflow wording so investigation remains an optional task-specific helper Pod tactic rather than a configured fixed Ticket role.
- Preserved `intake` and existing planning/ready/queued/inprogress/done workflow semantics.

Implementation:
- Coder commit: `427a270 ticket: remove fixed investigator role`
- Merge commit: `b552372 merge: remove fixed investigator ticket role`
- Reviewer: approved by `reviewer-remove-fixed-investigator-ticket-role`.

Validation:
- Coder/reviewer focused role/config/panel tests passed.
- Post-merge validation passed:
  - `cargo fmt --check`
  - `git diff --check`
  - `cargo run -q -p yoi -- ticket doctor`
  - `cargo check --workspace`
  - `nix build .#yoi`
