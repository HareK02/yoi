---
title: "Built-in Ticket intake and orchestration routing"
state: "closed"
created_at: "2026-06-01T03:12:52Z"
updated_at: "2026-06-05T06:42:40Z"
---

## Background

Yoi needs a durable project-level coordination record that Intake, Orchestrator, coder Pods, reviewer Pods, and humans can share. The accepted concept name is **Ticket**.

A Ticket is not the same thing as the session-local Task tool. A Task tracks short-lived user-visible progress inside a Pod/session. A Ticket is a durable orchestration contract with requirements, decisions, plans, implementation reports, reviews, artifacts, and resolution history.

The current repository already stores this information under `work-items/` and manages it through `tickets.sh`. For now, that storage format remains the local backend. The new code-facing concept should be `Ticket`, not `WorkItem`; `work-items/` is a current local storage path, not the product concept name.

The original version of this ticket mixed several layers: local ticket backend, built-in Pod tools, Intake behavior, Orchestrator routing, action-required state, and scheduler-like automation. That is too broad for a single implementation ticket, so this ticket now acts as the umbrella tracking ticket for the split.

## Terminology

- `Ticket`: durable orchestration record shared by Intake, Orchestrator, implementation Pods, reviewer Pods, and humans.
- `Task`: session-local progress tracking tool; intentionally separate from Ticket.
- `Assignment`: a concrete delegation from Orchestrator to a coder/reviewer/investigator Pod.
- `IntentPacket`: short implementation/review contract derived from a Ticket and passed to an Assignment.
- `TicketBackend`: backend abstraction for reading and mutating Tickets.
- `LocalTicketBackend`: backend implementation for the current `work-items/` directory format.

## Scope of the umbrella

This umbrella covers the following sequence:

1. `ticket-local-files-backend`
   - Add a code-facing Ticket domain model and local backend for current `work-items/` files.
   - Keep `tickets.sh` and existing file format compatible.

2. `ticket-built-in-feature-tools`
   - Expose Ticket operations as a built-in Pod feature/tool surface.
   - Treat Ticket operations as typed backend authority, not arbitrary filesystem writes.

3. `ticket-intake-workflow`
   - Define and install an Intake workflow/profile that clarifies user requests and creates Tickets after user agreement.

4. `ticket-orchestrator-routing`
   - Let Orchestrator classify Tickets and route them into preflight, implementation, review, spike, blocked/action-required, or close paths.

Later scheduler/lease/automatic maintainer behavior is out of scope for this umbrella until the above pieces are usable.

## Requirements

- Use `Ticket` as the product/code concept name.
- Keep existing `work-items/` storage as the LocalTicketBackend path for now.
- Do not rename the storage directory in this umbrella.
- Keep git history + ticket files authoritative.
- Preserve compatibility with `tickets.sh` until a replacement is explicitly accepted.
- Keep Ticket authority separate from delegated filesystem write scope.
- Avoid exposing arbitrary file writes through Ticket tools.
- Keep Intake focused on clarification and Ticket creation/update; it must not schedule implementation itself.
- Keep Orchestrator responsible for routing/scheduling decisions, not Intake.
- Do not introduce an unattended scheduler in the MVP.

## Acceptance criteria

- This umbrella records the Ticket terminology decision and split plan.
- Child tickets exist for:
  - local files backend;
  - built-in feature tools;
  - intake workflow;
  - orchestrator routing.
- Child tickets state their dependencies, scope, non-goals, and acceptance criteria clearly enough for preflight/implementation sequencing.
- Historical `WorkItem` wording in this ticket is either removed or explicitly described as old terminology.
- `tickets.sh doctor` passes after the split.

## Non-goals

- Renaming `work-items/` to `tickets/`.
- Replacing `tickets.sh` immediately.
- Building a scheduler/lease system.
- Building a TUI spawned-Pod panel.
- Changing the session-local Task tool.
- Integrating with GitHub Issues, Linear, MCP, or external trackers in this umbrella.
