# Clarification

The requested command is not a TUI attach/switch affordance. It should initiate a Pod-authoritative peer handshake between the currently attached Pod and another existing Pod.

The intended result is broader than one-sided `ListPods` visibility: after a successful handshake, both Pods should be mutually visible as peers and should be able to exchange messages through an explicit peer-safe tool/method. The relationship must stay distinct from spawned-child delegation: no delegated filesystem scope, no parent/child ownership, no child completion notifications, and no child output cursor authority.

The TUI `:` command is the human-facing control path for initiating the handshake; the durable relationship and messaging authorization belong in Pod metadata/runtime semantics, not in TUI-local state.
