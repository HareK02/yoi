Merged and completed the workflow template non-goals cleanup.

Summary:
- Removed generic `Non-goals` / `Non-goals / constraints` workflow-template language from maintained workflow templates and IntentPacket examples.
- Replaced broad exclusion buckets with `Binding decisions / invariants`, `Implementation latitude`, escalation-focused language, and concrete authority-boundary wording.
- Removed the broad Intake prompt asking `やらないことは何か。` and kept concrete exclusions only when they are binding decisions or authority/scope boundaries.
- Updated Ticket role prompt guidance and development work-item docs to match the maintained workflow language.
- Historical Ticket artifacts/threads were not rewritten.

Merged branch/worktree:
- Branch: `remove-non-goals-workflow-templates`
- Commit: `783cd42 workflow: replace non-goals template language`
- Merge commit on `develop`: `90870f7 merge: workflow template non-goals cleanup`

Validation passed after merge:
- `cargo fmt --check`
- `cargo test -p client ticket_role`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/remove-non-goals-workflow-templates`.
- Deleted branch `remove-non-goals-workflow-templates`.