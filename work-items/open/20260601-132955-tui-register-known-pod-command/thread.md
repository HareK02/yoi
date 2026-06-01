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
