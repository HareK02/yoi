Implemented, reviewed, merged, and validated.

Summary:
- Added strict Ticket role launch readiness validation while preserving backend config loading/list/show behavior.
- Role launch planning now rejects missing role table, missing role `profile`, explicit top-level `profile = "inherit"`, and unresolvable concrete selectors before spawn.
- `plan_ticket_role_launch(_with_config)` uses launch-specific validation instead of implicit role defaults.
- No implicit builtin/default/inherit fallback was added.
- `TicketRoleLaunchPlan::spawn_config` still defensively rejects `inherit`.
- Diagnostics are bounded/actionable for Panel/launcher surfaces.
- Init/scaffold generation was intentionally not implemented here; `ticket-init-role-profile-scaffold` remains the next task.

Implementation:
- Child commit: `30a07b7 ticket: validate role launch config`
- Merge commit: `merge: role launch config validation`

Review:
- External reviewer `role-launch-config-validation-reviewer-20260607` approved with no blockers.

Validation after merge:
- `cargo test -p ticket config --lib`
- `cargo test -p client ticket_role --lib`
- `cargo test -p tui multi_pod --lib`
- `cargo fmt --check`
- `git diff --check`
- `target/debug/yoi ticket doctor`