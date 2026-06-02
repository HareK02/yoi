# Implementation report: peer Pod handshake command

Date: 2026-06-02

## Investigation

The current boundaries are documented in `artifacts/investigation-summary.md`. No escalation blocker was found. The main concern identified was avoiding reuse of spawned-child state (`SpawnedPodRegistry`, delegated scope, output cursors, and child completion semantics) for peer communication; the implementation therefore adds separate peer metadata and a separate peer send tool.

## Implemented behavior

- Added reciprocal peer metadata to `PodMetadata` as `peers`, separate from `spawned_children` and `reclaimed_children`.
- Added protocol `Method::RegisterPeer { name }` and `Event::PeerRegistered { result }`.
- Added controller handling for `RegisterPeer`, idle/paused only, validating an existing target Pod and rejecting self-handshakes.
- Added `PodDiscovery::register_peer` that persists both metadata directions and rolls back the first side on ordinary second-side write failure.
- Extended `ListPods` visibility to include `VisibilityReason::Peer`; a successful handshake makes both Pods see each other as `peer` through Pod metadata.
- Added `SendToPeerPod` as a distinct LLM tool. It only sends to visible live peer Pods, delivers `Method::Notify` with a source label, and does not use child delegation, output cursors, parent ownership, or child completion notifications.
- Added TUI command `:peer <pod-name>` for idle attached Pods. Success is reported through a transient actionbar notice when the controller returns `PeerRegistered`.
- Documented peer semantics in `docs/design/pod-session-state.md` and added prompt guidance that peer Pods are not spawned children.

## Tests and validation run

- `cargo test -p protocol -p pod-store -p pod -p tui --lib`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`

## Notes

The two-file reciprocal metadata update is not crash-transactional because the existing Pod metadata store has no multi-record transaction boundary. The implementation avoids successful replies with one-sided state for normal validation/write failures by rolling back the first write if the reciprocal write fails.
