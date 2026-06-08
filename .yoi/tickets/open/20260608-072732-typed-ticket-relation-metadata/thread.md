<!-- event: create author: LocalTicketBackend at: 2026-06-08T07:27:32Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-08T07:28:29Z -->

## Decision

## Design consideration: Queue gate should account for unresolved dependencies

This relation metadata should be available before Orchestrator planning. That implies a stronger queue-time question:

If a Ticket has a project-level dependency such as `depends_on: X`, and `X` is still in `planning` / not ready / not resolved, should the user be prevented or warned before Queueing the dependent Ticket?

This needs explicit design before implementation.

Considerations:

- A dependency that is still `planning` likely means the dependent Ticket is not truly runnable yet.
- Queue should probably reject or strongly warn when unresolved `depends_on` / blocking relations remain.
- The behavior may differ by relation kind:
  - `depends_on` unresolved: likely hard block or confirmation-required;
  - `blocks`: informational for the blocked Ticket;
  - `related`: never blocks;
  - `parent` / `child`: depends on workflow semantics;
  - `supersedes` / `duplicate_of`: may redirect or discourage Queue.
- The Panel should be able to show why Queue is unavailable or risky before involving the Orchestrator.
- CLI Queue/state transitions should enforce the same rule or at least expose the same diagnostics.
- Orchestrator should still re-check relations at acceptance time, because relations may change between Queue and routing.

The implementation should decide whether unresolved dependencies make Queue invalid, require confirmation, or allow Queue with a visible blocked state. The important requirement is that dependency relation metadata is not only an Orchestrator input; it should also inform the human Queue gate.

---
