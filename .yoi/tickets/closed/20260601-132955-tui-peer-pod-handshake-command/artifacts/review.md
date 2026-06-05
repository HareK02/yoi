# Review: peer Pod handshake command

Reviewer Pods:

- Initial review: `peer-pod-handshake-reviewer-20260602`
- Re-review: `peer-pod-handshake-rereviewer-20260602`

## Result

Approved after fixes. No remaining blockers.

## Initial blocker findings

The first reviewer found two blockers:

1. `SendToPeerPod` wrote `Method::Notify` directly to the target socket without draining connect-time `Alert` / `Snapshot` traffic, so a real Pod socket could fail to process the notify while the sender reported success.
2. Reciprocal registration rollback could delete a pre-existing `source -> target` peer relation when the reciprocal `target -> source` write failed.

## Fix verification

Coder added commit `057c2ef fix: harden peer pod registration`.

The re-review confirmed:

- `SendToPeerPod` now uses the existing `connect_and_send` one-shot client path, which drains connect-time `Alert` / `Snapshot` traffic before writing `Method::Notify` and reports delivery failures.
- A regression test covers a target socket that emits an alert/snapshot before reading the peer notify.
- `register_peer` now snapshots the source Pod's existing peer list and restores that exact list if the target-side write fails.
- A regression test covers an injected target-side failure where the source already had the peer relation and verifies it is preserved.

## Validation evidence

Coder reported:

- `cargo test -p pod discovery --lib`
- `cargo test -p tui command --lib`
- `cargo test -p protocol -p pod-store -p pod -p tui --lib`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`

Reviewer re-ran:

- `cargo test -p pod --lib peer`
- `cargo test -p pod --lib connect_and_send`

Parent/orchestrator reran after merge:

- `cargo test -p pod discovery --lib`
- `cargo test -p tui command --lib`
- `cargo test -p protocol -p pod-store -p pod -p tui --lib`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`
- `./result/bin/yoi pod --help`

## Residual risk

Peer registration updates two metadata files and is not crash-transactional because the current Pod metadata store has no multi-record transaction boundary. Normal write failures are handled by restoring the exact prior source peer list.

The implemented peer relationship is metadata-level reciprocal registration, not live target-process consent. This is documented and remains separate from spawned-child delegation, delegated scope, output cursors, parent ownership, and child completion notifications.
