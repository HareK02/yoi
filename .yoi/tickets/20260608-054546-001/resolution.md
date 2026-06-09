Merged and completed the Intake Pod idle-shutdown behavior.

Summary:
- Ticket role launcher now passes a hidden process-local `--ticket-role <role>` marker to Pod runtime for Ticket role Pods.
- `TicketIntakeReady` successful tool results schedule transient shutdown-after-idle only for runtime role `intake`.
- Failed `TicketIntakeReady`, non-Intake roles, and other tools do not schedule shutdown.
- Controller executes ordinary clean shutdown only after run completion/final response/history+session commit/status transition and only when the Pod is Idle.
- The shutdown request is transient runtime state and does not mutate or remove local Ticket claim/session records.

Merged branch/worktree:
- Branch: `shutdown-intake-pod-after-ready-idle`
- Commit: `61c3231 pod: stop intake after ready idle`
- Merge commit on `develop`: `f7c5060 merge: shutdown intake after ready idle`

Validation passed after merge:
- `cargo test -p pod shutdown_after_idle --lib`
- `cargo test -p client runtime_args --lib`
- `cargo test -p client ticket_role --lib`
- `cargo test -p pod --lib`
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/shutdown-intake-pod-after-ready-idle`.
- Deleted branch `shutdown-intake-pod-after-ready-idle`.

Residual notes:
- Hidden `--ticket-role` is not an authorization boundary; manual opt-in affects only the current Pod's self-shutdown scheduling and grants no Ticket or Pod-control authority.
- Generic restore of an old Intake role session does not automatically regain the process-local role marker unless launched through the Ticket role path; this matches the non-durable shutdown signal boundary.