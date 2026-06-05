---
id: 20260605-040104-ticket-built-in-feature-tools
slug: ticket-built-in-feature-tools
title: Ticket built-in feature tools
status: open
kind: task
priority: P1
labels: [ticket, feature, tool, orchestration]
created_at: 2026-06-05T04:01:04Z
updated_at: 2026-06-05T05:58:39Z
assignee: null
legacy_ticket: null
---

## Background

After a typed Ticket backend exists, Pods need a controlled way to read and mutate Tickets without receiving arbitrary filesystem write scope over the repository.

Ticket operations are orchestration authority, not ordinary source-file writes. A coder Pod may have write access only to a child worktree and still need to report implementation progress or attach artifacts to a Ticket through a typed operation. Intake and Orchestrator Pods likewise need Ticket operations that are bounded, auditable, and backend-independent.

This ticket depends on `ticket-local-files-backend`. It should also be sequenced after or alongside `feature-api-authority-separation` if that ticket is still open, because Ticket tools should use the built-in contribution path without confusing internal built-ins with external plugin grants.

## Requirements

- Add a built-in Pod feature for Ticket tools.
- Register tools through the existing feature contribution / ToolRegistry path.
- Use the typed Ticket backend from `ticket-local-files-backend`.
- Treat Ticket operations as configured backend authority, not filesystem scope.
- Do not grant arbitrary writes to `work-items/` through generic filesystem tools.
- Keep tool outputs bounded and model-readable.
- Preserve current `tickets.sh` semantics where equivalent operations exist.
- Ensure Ticket operations are recorded durably in the ticket files/thread/resolution/artifacts.
- Keep permission/policy behavior explicit and fail-closed.
- Avoid external plugin loading or MCP integration in this ticket.

## Candidate tool surface

Exact names can be adjusted during preflight, but the surface should be close to:

- `TicketCreate`
- `TicketList`
- `TicketShow`
- `TicketComment`
- `TicketReview`
- `TicketStatus`
- `TicketClose`
- `TicketDoctor`

Optional follow-up if needed rather than MVP:

- `TicketArtifactWrite`
- `TicketArtifactRead`
- `TicketSearch`

## Authority model

- The Pod must receive a Ticket backend/tool grant through manifest/profile/feature configuration.
- The grant points to a configured backend root, initially the local repository `work-items/` root.
- Tool implementations validate paths and identifiers against the backend root.
- Artifact writes are constrained to the selected ticket's `artifacts/` directory.
- Ticket tools do not imply general `Write` permission to the repository.
- Existing PreToolCall policy still applies to the tool call itself.

## Non-goals

- Implementing the local files backend itself.
- Building Intake workflow behavior.
- Building Orchestrator routing/scheduling.
- Renaming `work-items/`.
- Removing `tickets.sh`.
- External tracker integration.
- MCP/plugin loading.
- Scheduler/lease/queue automation.
- TUI changes.

## Acceptance criteria

- Ticket tools are registered only when the built-in Ticket feature is enabled/configured.
- Ticket tools operate through the typed Ticket backend and not shelling out to `tickets.sh`.
- A Pod without filesystem write scope to `work-items/` can perform allowed Ticket operations through the tool grant.
- A Pod without the Ticket tool grant cannot mutate Tickets through these tools.
- Tool schemas and outputs are bounded, deterministic enough for tests, and suitable for LLM use.
- Ticket create/comment/review/status/close operations produce files accepted by `./tickets.sh doctor`.
- Tool calls produce no writes outside the backend root and selected ticket artifact directories.
- Unit/integration tests cover allowed/denied operation paths and path traversal attempts.
- `cargo test` for affected crates passes.
- `cargo check --workspace --all-targets`, `cargo fmt --check`, `git diff --check`, and `./tickets.sh doctor` pass.

## Dependencies

- Requires `ticket-local-files-backend`.
- Prefer after `feature-api-authority-separation` if that ticket remains open.

## Follow-up tickets

- `ticket-intake-workflow`
- `ticket-orchestrator-routing`
