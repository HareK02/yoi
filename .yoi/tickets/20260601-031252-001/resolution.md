Built-in Ticket intake and orchestration routing umbrella is complete.

This umbrella was split into four implementation-sized child tickets, all of which are now closed:

1. `ticket-local-files-backend`
   - Added the low-level `ticket` crate, typed Ticket domain/backend, and `LocalTicketBackend` over current `work-items/` files.

2. `ticket-built-in-feature-tools`
   - Added built-in Ticket tools through typed backend authority and the Pod feature contribution path.
   - Kept Ticket tool behavior in `crates/ticket` and Pod as a thin feature adapter.

3. `ticket-intake-workflow`
   - Added `.yoi/workflow/ticket-intake-workflow.md` for clarifying user requests and materializing agreed Tickets after user approval.

4. `ticket-orchestrator-routing`
   - Added `.yoi/workflow/ticket-orchestrator-routing.md` for classifying Tickets into requirements sync, preflight, spike, implementation, review, blocked/action-required, close-ready, defer/pending, or no-op paths.

Terminology decision:

- `Ticket` is the durable orchestration record concept.
- `Task` remains session-local progress tracking.
- `Assignment` is concrete Pod delegation.
- `IntentPacket` is the implementation/review contract derived from a Ticket.
- `work-items/` remains the current LocalTicketBackend storage path for now.

Scope preserved:

- The storage directory was not renamed.
- `tickets.sh` remains compatible and in use.
- Ticket authority is separate from delegated filesystem write scope.
- Intake does not schedule implementation.
- Orchestrator routing does not introduce an unattended scheduler/lease/queue system.
- TUI spawned-Pod panel work was deprioritized and is not part of this path.

Validation:

- Child tickets were independently reviewed and validated.
- Final child workflow validation passed `git diff --check` and `./tickets.sh doctor`.
- `./tickets.sh doctor` passes for the umbrella close.

Historical note:

Older thread entries and artifacts may contain superseded `WorkItem` or old system-name wording as historical context. The current terminology and implementation use `Ticket`.
