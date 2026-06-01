# Clarification

The requested command is not a TUI attach/switch affordance. It should make another existing Pod known to the currently attached Pod so that the current Pod's `ListPods` tool can see it.

The implementation should therefore focus on Pod-authoritative visibility metadata and `ListPods` semantics, with the TUI `:` command only acting as the human-facing control path that registers that relationship.
