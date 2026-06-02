---
id: 20260601-132955-tui-peer-pod-handshake-command
slug: tui-peer-pod-handshake-command
title: TUI: add peer Pod handshake and messaging command
status: open
kind: task
priority: P2
labels: [tui, pod, command, orchestration]
created_at: 2026-06-01T13:29:55Z
updated_at: 2026-06-02T10:17:42Z
assignee: null
legacy_ticket: null
---

## Background

When a user is attached to one running Pod, there is currently no user-facing way to introduce that Pod to another existing Pod as a peer. Existing visibility is mostly self/spawned-child/current-state based, so the current Pod's `ListPods` tool does not represent arbitrary peer Pods and `SendToPod` is effectively limited to previously spawned children.

The desired `:` command is not a TUI attach/switch command. It should initiate a Pod-authoritative peer handshake so the currently attached Pod and a target Pod can become mutually visible and can exchange messages through the existing Pod tooling surface or a new explicitly peer-safe messaging surface.

This relationship must be separate from spawned-child delegation. A peer handshake should not imply filesystem scope delegation, parent ownership, child completion notifications, or child output cursor authority.

## Requirements

- Add a TUI command-mode entry point for initiating a peer Pod handshake, tentatively `:peer <name>`, `:know-pod <name>`, `:link-pod <name>`, or similar.
- The command should send an explicit Pod/runtime method to the currently attached Pod; it must not be a TUI-only local list mutation.
- Add protocol/runtime support for registering peer visibility between two existing Pods.
- The relationship should be reciprocal by default: after a successful handshake, both Pods should be able to see each other through `ListPods` with a source/state label that identifies the relationship as peer/known, not spawned child.
- The durable effect should live in Pod current-state metadata or an equivalent Pod-authoritative visibility record so it survives reconnect/restore where appropriate.
- `ListPods` from either peer should include the other Pod after the handshake succeeds.
- Extend messaging semantics so a Pod can send a message to a visible live/restorable peer, not only to a spawned child. This may reuse `SendToPod` if its contract is broadened safely, or introduce a distinct peer-send tool/method if keeping spawned-child semantics separate is clearer.
- A peer message should be delivered as an explicit user-visible/control-plane message according to the chosen messaging semantics; it must not silently mutate hidden model context.
- Peer messaging must not grant delegated filesystem scope, output cursor authority, parent/child lifecycle authority, or completion-notification authority.
- The command should resolve the target by Pod name using existing Pod metadata/live registry visibility that is available to the controller/human, then perform a safe handshake with the target Pod.
- Handle at least:
  - target Pod is live and can accept the handshake;
  - target Pod is stopped but restorable;
  - target Pod name is unknown or ambiguous;
  - target Pod rejects or cannot persist the reciprocal relationship;
  - current TUI is not attached to a Pod;
  - current Pod is in a state where the handshake method cannot be safely delivered.
- Define whether a one-sided fallback is allowed when reciprocal registration fails. If allowed, it must be clearly labeled and must not imply mutual messaging.
- The current Pod's model conversation history must not be polluted merely because the user registers a peer. The handshake is Pod metadata/control state, not a user message to the model.
- Message delivery, however, must be recorded through the normal history/event path appropriate for delivered messages so later turns can explain why the receiver saw the message.
- Provide clear command diagnostics/actionbar feedback.
- Add focused tests for command parsing, protocol/runtime method handling, metadata persistence/restore, reciprocal `ListPods` visibility behavior, and peer messaging authorization/delivery. If full live TUI/socket E2E is not feasible, document the manual validation path.

## Non-goals

- Do not implement arbitrary autonomous Pod-to-Pod scheduling or background chatter.
- Do not switch the TUI's active attachment as the primary behavior of this command.
- Do not replace the multi-Pod dashboard or Pod picker.
- Do not treat registered peer Pods as spawned children.
- Do not make child completion notifications authoritative.
- Do not add delegated scope or reclaim behavior for peer Pods unless a later design explicitly requires it.
- Do not add a broad social graph or arbitrary persistent relationship model beyond the minimal peer visibility/messaging relationship needed here.

## Acceptance criteria

- A user can invoke a documented `:` command from a single-Pod TUI to handshake the current Pod with another existing Pod.
- After successful handshake, each Pod's `ListPods` tool output includes the other Pod with a state/source label that does not misrepresent it as a spawned child.
- A Pod can send a message to the registered peer through the chosen tool/method, and the receiver records the delivered message through the appropriate durable path.
- `RestorePod` or other visibility-scoped Pod tools can operate on registered peers according to the existing state-aware rules.
- The peer relationship is persisted in Pod-authoritative current state and survives ordinary TUI reconnect/Pod restore where both Pods remain visible/restorable.
- Failure modes produce clear diagnostics and do not modify history or create partial misleading visibility records.
- Tests cover parser/model/runtime visibility and peer messaging behavior, and implementation notes record any manual validation required for live TUI behavior.
- Documentation/help text for TUI commands, Pod visibility semantics, and peer messaging semantics is updated.
- Validation includes relevant TUI/client/protocol/pod-store/pod tests, `./tickets.sh doctor`, `git diff --check`, and `nix build .#yoi` unless explicitly deferred with rationale.
