<!-- event: create author: tickets.sh at: 2026-06-01T13:29:55Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-01T16:19:25Z -->

## Decision

# Clarification

The requested command is not a TUI attach/switch affordance. It should make another existing Pod known to the currently attached Pod so that the current Pod's `ListPods` tool can see it.

The implementation should therefore focus on Pod-authoritative visibility metadata and `ListPods` semantics, with the TUI `:` command only acting as the human-facing control path that registers that relationship.


---

<!-- event: decision author: hare at: 2026-06-02T10:16:21Z -->

## Decision

# Clarification

The requested command is not a TUI attach/switch affordance. It should initiate a Pod-authoritative peer handshake between the currently attached Pod and another existing Pod.

The intended result is broader than one-sided `ListPods` visibility: after a successful handshake, both Pods should be mutually visible as peers and should be able to exchange messages through an explicit peer-safe tool/method. The relationship must stay distinct from spawned-child delegation: no delegated filesystem scope, no parent/child ownership, no child completion notifications, and no child output cursor authority.

The TUI `:` command is the human-facing control path for initiating the handshake; the durable relationship and messaging authorization belong in Pod metadata/runtime semantics, not in TUI-local state.


---

<!-- event: plan author: hare at: 2026-06-02T10:17:42Z -->

## Plan

# Delegation intent: peer Pod handshake and messaging

Intent:
- Investigate the existing Pod visibility/messaging implementation, then implement a TUI-initiated peer Pod handshake and peer messaging path if no design blockers are found.

Requirements:
- Start with investigation. Map current authority boundaries for `ListPods`, `RestorePod`, `SendToPod`, Pod metadata, spawned-child registry, protocol methods, and TUI `:` command handling.
- If there are design blockers or ambiguous authority decisions, write findings to the ticket artifacts and stop for parent decision.
- If there are no blockers, proceed with implementation in the worktree.
- Add a TUI `:` command that initiates peer handshake from the currently attached Pod to a target Pod by name.
- Make the peer relationship Pod-authoritative, durable where appropriate, and reciprocal by default.
- Ensure both Pods see each other in `ListPods` as peer/known, not spawned child.
- Add or safely broaden messaging semantics so a Pod can message a visible peer without granting spawned-child powers.
- Record delivered peer messages through an explainable durable path; do not silently mutate hidden context.
- Preserve explicit boundaries: no delegated filesystem scope, parent/child ownership, completion notification authority, or child output cursor authority for peer Pods.
- Add focused tests for protocol/runtime behavior, metadata persistence/restore, reciprocal `ListPods`, peer message authorization/delivery, and TUI command parsing/help where feasible.
- Update docs/help text for the new command and peer semantics.

Invariants:
- Do not treat peer Pods as spawned children.
- Do not reintroduce hidden context injection; model-affecting delivered messages must go through the normal committed history/event path.
- Do not add broad relationship graph semantics beyond the minimal peer relation.
- Do not edit the parent workspace; work only in the delegated worktree.
- Do not read ignored secret-like file contents.
- Do not close the ticket, merge the branch, delete worktrees, or push.

Non-goals:
- Arbitrary autonomous Pod-to-Pod background chatter.
- Replacing the multi-Pod dashboard or Pod picker.
- Delegated scope sharing between peers.
- E2E test framework design for real spawned processes.

Escalate if:
- Reciprocal handshake cannot be made atomic enough to avoid misleading partial state.
- Peer messaging requires changing history semantics in a way that could violate context/history invariants.
- Existing protocol or metadata schemas need a broad migration.
- The TUI command cannot safely deliver the runtime method while preserving current Pod authority.
- `SendToPod` broadening would confuse spawned-child and peer semantics.

Validation:
- Before implementation: write a short investigation summary in the implementation report or a separate artifact.
- Focused tests for touched crates.
- `./tickets.sh doctor` and `git diff --check`.
- `nix build .#yoi` if feasible; record if skipped and why.
- Commit the implementation in the worktree when reviewable.


---

<!-- event: implementation_report author: hare at: 2026-06-02T10:42:37Z -->

## Implementation report

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


---

<!-- event: implementation_report author: hare at: 2026-06-02T13:18:34Z -->

## Implementation report

# Implementation report: peer Pod handshake command

Date: 2026-06-02

## Investigation

The current boundaries are documented in `artifacts/investigation-summary.md`. No escalation blocker was found. The main concern identified was avoiding reuse of spawned-child state (`SpawnedPodRegistry`, delegated scope, output cursors, and child completion semantics) for peer communication; the implementation therefore adds separate peer metadata and a separate peer send tool.

## Implemented behavior

- Added reciprocal peer metadata to `PodMetadata` as `peers`, separate from `spawned_children` and `reclaimed_children`.
- Added protocol `Method::RegisterPeer { name }` and `Event::PeerRegistered { result }`.
- Added controller handling for `RegisterPeer`, idle/paused only, validating an existing target Pod and rejecting self-handshakes.
- Added `PodDiscovery::register_peer` that persists both metadata directions and restores the exact prior source-side peer state on ordinary second-side write failure.
- Extended `ListPods` visibility to include `VisibilityReason::Peer`; a successful handshake makes both Pods see each other as `peer` through Pod metadata.
- Added `SendToPeerPod` as a distinct LLM tool. It only sends to visible live peer Pods, delivers `Method::Notify` with a source label, and does not use child delegation, output cursors, parent ownership, or child completion notifications.
- Added TUI command `:peer <pod-name>` for idle attached Pods. Success is reported through a transient actionbar notice when the controller returns `PeerRegistered`.
- Documented peer semantics in `docs/design/pod-session-state.md` and added prompt guidance that peer Pods are not spawned children.

## Reviewer blocker fixes

- `SendToPeerPod` now reuses the existing one-shot Pod socket client path (`connect_and_send`), which drains connect-time `Alert` / `Snapshot` traffic before writing `Notify` and returns an error if method delivery fails.
- Added a regression test where the target socket emits an alert and snapshot before reading the peer `Notify`, proving the peer send drains the prelude and still delivers the message.
- Registration failure rollback now restores the exact prior source peer list instead of unconditionally removing `source -> target`; a target-side injected failure test verifies a pre-existing source relation is preserved.
- Wording now describes `:peer` as metadata-level reciprocal registration rather than live target-controller consent, and documents that `SendToPeerPod` fails for non-live peers instead of auto-restoring them.

## Tests and validation run

- `cargo test -p protocol -p pod-store -p pod -p tui --lib`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`

## Notes

The two-file reciprocal metadata update is not crash-transactional because the existing Pod metadata store has no multi-record transaction boundary. The implementation avoids successful replies with one-sided state for normal validation/write failures by restoring the exact prior source-side peer list if the reciprocal write fails.


---

<!-- event: review author: hare at: 2026-06-02T13:54:50Z status: approve -->

## Review: approve

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


---

<!-- event: close author: hare at: 2026-06-02T13:54:50Z status: closed -->

## Closed

Implemented TUI-initiated peer Pod handshake and peer messaging. Peers are persisted separately from spawned children, appear in ListPods as peers, and can receive peer messages through a distinct tool/method. Reviewer blockers were fixed; focused tests and nix build passed.


---
