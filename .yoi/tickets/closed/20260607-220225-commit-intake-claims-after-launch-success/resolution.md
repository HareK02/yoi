Merged and completed the Intake claim commit timing fix.

Summary:
- Existing-Ticket Intake durable claim persistence now happens only after `launch_ticket_role_pod_with_options(...).await?` returns successfully with launch/run acceptance evidence.
- Pre-launch durable `claim_ticket(...)` was removed from `prepare_existing_ticket_intake_launch`.
- Deferred `IntakeRegistryUpdate::ClaimTicket` is committed by `commit_intake_registry_update(...)` after launch success.
- Existing live/restorable claims still block duplicate Intake launch before launch preparation.
- In-flight duplicate protection remains transient via `self.sending`; it is not a durable claim.
- Peer registration/handoff ordering is preserved.

Merged branch/worktree:
- Branch: `commit-intake-claims-after-launch-success`
- Commit: `6797be3 fix: defer intake claim until launch acceptance`
- Merge commit on `develop`: `3bc4ab2 merge: defer intake claims until launch success`

Validation passed after merge:
- `cargo test -p tui role_session_registry --lib`
- `cargo test -p tui multi_pod --lib`
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/commit-intake-claims-after-launch-success`.
- Deleted branch `commit-intake-claims-after-launch-success`.

Residual note:
- There is still no full mocked/integration test that drives spawn/connect/run-acceptance failure through `launch_intake_with_handoff` and then asserts registry emptiness. Reviewer accepted the explicit `await?` before registry commit and focused tests as sufficient for this Ticket.