Implemented, reviewed, merged, validated, and applied to this workspace config.

Summary:
- Added a narrow `yoi ticket init` command.
- The command creates `.yoi/ticket.config.toml` only when missing and refuses to overwrite an existing config with an actionable diagnostic.
- Added `ticket_config_scaffold()` for the generated config body.
- Generated config includes explicit backend config and fixed role tables for intake, orchestrator, coder, reviewer, and investigator.
- Each generated role uses explicit `profile = "builtin:default"` and explicit default workflow.
- No runtime fallback was added or loosened.
- The generated config validates for Ticket role launch planning under the strict validation implemented by `ticket-role-launch-config-strict-validation`.
- This repository's `.yoi/ticket.config.toml` was updated to the scaffolded explicit role-profile shape for dogfooding.

Implementation:
- Child commit: `f265098 ticket: add ticket config init scaffold`
- Merge commit: `merge: ticket init scaffold`

Review:
- External reviewer `ticket-init-scaffold-reviewer-20260607` approved with no blockers.

Validation after merge:
- `cargo test -p ticket config --lib`
- `cargo test -p client ticket_role --lib`
- `cargo test -p yoi ticket`
- `cargo fmt --check`
- `git diff --check`
- `target/debug/yoi ticket doctor`