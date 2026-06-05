---
id: 20260605-040104-ticket-local-files-backend
slug: ticket-local-files-backend
title: Ticket local files backend
status: open
kind: task
priority: P1
labels: [ticket, backend, orchestration]
created_at: 2026-06-05T04:01:04Z
updated_at: 2026-06-05T04:24:20Z
assignee: null
legacy_ticket: null
---

## Background

The first step toward Ticket-driven multi-agent orchestration is to make the current `work-items/` + `tickets.sh` file format accessible through a typed Rust API.

The product/code concept name is **Ticket**. The current directory name `work-items/` remains the LocalTicketBackend storage path for now; do not rename it in this ticket.

This ticket should produce a backend-shaped domain layer that future Pod tools, Intake workflows, Orchestrator routing, TUI views, and external tracker integrations can use without depending on `tickets.sh` shell execution or ad hoc markdown parsing.

## Requirements

- Add a code-facing Ticket domain/backend layer.
  - Preferred crate: `crates/ticket` or `crates/tickets`; choose the name that reads best in Rust and avoids collision with `tickets.sh` scripts.
  - The public concept/type names should use `Ticket`, not `WorkItem`.
- Implement `LocalTicketBackend` over current `work-items/` storage.
- Preserve the current markdown + frontmatter + `thread.md` + `artifacts/` layout.
- Preserve compatibility with `tickets.sh`; existing script operations should continue to work on files written by the Rust backend.
- Keep git history and repository files authoritative.
- Implement typed operations equivalent to at least:
  - list;
  - show;
  - create;
  - comment / plan / decision / implementation_report events;
  - review approve/request-changes;
  - status transition;
  - close with resolution;
  - doctor/consistency check.
- Use a thin typed envelope with Markdown/freeform bodies.
- Include machine-readable fields needed by Orchestrator/TUI/policy:
  - id / slug / title;
  - kind / priority / labels;
  - status;
  - readiness;
  - needs-preflight;
  - risk flags;
  - action-required state;
  - event kind;
  - references to files, branches, commits, Pods, artifacts, URLs where practical.
- Keep enums extensible where backend compatibility needs raw values.
- Implement safe local-file mutation:
  - atomic writes where files are replaced;
  - lock/conflict handling sufficient for multiple Pod/tool users;
  - no writes outside the configured local ticket root except controlled temp files.
- Provide bounded errors/diagnostics suitable for later tool output.

## Suggested API shape

The exact API can be adjusted during implementation, but should remain backend-shaped:

```rust
trait TicketBackend {
    fn list(&self, filter: TicketFilter) -> Result<Vec<TicketSummary>>;
    fn show(&self, id: TicketIdOrSlug) -> Result<Ticket>;
    fn create(&self, input: NewTicket) -> Result<TicketRef>;
    fn add_event(&self, id: TicketIdOrSlug, event: NewTicketEvent) -> Result<()>;
    fn review(&self, id: TicketIdOrSlug, review: TicketReview) -> Result<()>;
    fn set_status(&self, id: TicketIdOrSlug, status: TicketStatus) -> Result<()>;
    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> Result<()>;
    fn doctor(&self) -> Result<TicketDoctorReport>;
}
```

## Non-goals

- Exposing Pod tools.
- Implementing Intake Pod behavior.
- Implementing Orchestrator routing or scheduling.
- Renaming `work-items/`.
- Removing `tickets.sh`.
- Integrating GitHub Issues, Linear, Jira, MCP, or other external backends.
- Building a scheduler/lease system.
- Changing the session-local Task tool.

## Acceptance criteria

- Rust code can read existing open/pending/closed tickets from `work-items/`.
- Rust code can create a ticket equivalent to `tickets.sh create`.
- Rust code can append typed thread events equivalent to `tickets.sh comment` roles.
- Rust code can record approve/request-changes reviews equivalent to `tickets.sh review`.
- Rust code can close a ticket with a `resolution.md` equivalent to `tickets.sh close`.
- Rust doctor catches the same core consistency failures as `tickets.sh doctor` or clearly documents any remaining gaps.
- Files written by the Rust backend pass `./tickets.sh doctor`.
- Existing `tickets.sh` can still read/mutate tickets written by the Rust backend.
- Unit tests cover parsing, create, event append, review, status transition, close, doctor, lock/conflict handling, and corrupt input diagnostics.
- `cargo test` for the new crate passes.
- `cargo check --workspace --all-targets`, `cargo fmt --check`, `git diff --check`, and `./tickets.sh doctor` pass.

## Follow-up tickets

- `ticket-built-in-feature-tools`
- `ticket-intake-workflow`
- `ticket-orchestrator-routing`
