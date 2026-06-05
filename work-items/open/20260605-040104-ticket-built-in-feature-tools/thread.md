<!-- event: create author: tickets.sh at: 2026-06-05T04:01:04Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-05T05:37:41Z -->

## Decision

Decision: keep Ticket tool behavior mostly in the `ticket` crate and use `pod` only as a thin built-in feature adapter.

Rationale:

- `ticket` should own the Ticket domain/backend and the behavior of Ticket operations.
- `pod::feature` is still owned by the `pod` crate, so feature installation/authority wiring cannot be fully crate-contained without extracting a lower `feature-api` crate.
- Extracting `feature-api` is explicitly out of scope for this ticket.
- Putting Ticket tools in `crates/tools` would repeat the Task tool responsibility problem.

Dependency shape:

```text
ticket -> llm-worker   # Tool/ToolDefinition implementation only
pod -> ticket          # built-in feature adapter and backend-root wiring
```

Forbidden:

```text
ticket -> pod
```

Implementation direction:

- `crates/ticket` may add Ticket tool input/output types and Tool implementations.
- `crates/pod/src/feature/builtin/ticket.*` should only install/register those tools through `FeatureContribution` and resolve host/backend authority.
- Ticket tools must operate through typed `TicketBackend`/`LocalTicketBackend`, not through `tickets.sh` or generic filesystem writes.


---

<!-- event: plan author: hare at: 2026-06-05T05:37:42Z -->

## Plan

Preflight result: `implementation-ready`.

Prerequisites are complete:

- `ticket-local-files-backend` added the typed Ticket backend and `LocalTicketBackend`.
- `feature-api-authority-separation` clarified host-authority naming in `pod::feature`.

Scope for implementation:

- Add the MVP Ticket tool surface: create/list/show/comment/review/status/close/doctor.
- Keep Ticket tool behavior in `crates/ticket` and use a thin `pod` adapter for built-in feature registration and backend-root/authority wiring.
- Do not implement Intake workflow, Orchestrator routing, TUI changes, external trackers, MCP/plugin loading, scheduler/lease behavior, or `work-items/` rename.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---
