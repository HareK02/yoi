---
id: 20260605-190330-tui-ticket-role-actions
slug: tui-ticket-role-actions
title: TUI Ticket role actions
status: open
kind: task
priority: P1
labels: [tui, ticket, role, orchestration]
created_at: 2026-06-05T19:03:30Z
updated_at: 2026-06-05T19:06:17Z
assignee: null
legacy_ticket: null
---

## Background

After `ticket-role-pod-launcher` lands, TUI should expose simple Ticket-role actions without embedding launch/profile/prompt/workflow construction logic in UI code.

This ticket is for the TUI surface only. The launcher is responsible for reading `.yoi/ticket.config.toml`, selecting the Profile, constructing first-run input, and launching/sending to a role Pod. TUI should call that layer and present conservative, understandable actions.

## Goal

Add TUI actions for fixed Ticket roles using the shared Ticket role launcher.

The initial UI should favor explicit command/action entry over broad dashboard redesign. It should make it possible to start the relevant role Pod for a Ticket-oriented task while preserving human/orchestrator control.

## Requirements

- Depend on the shared `ticket-role-pod-launcher` API; do not duplicate launch construction in TUI.
- Support fixed Ticket role actions:
  - Intake / refine Ticket;
  - Route Ticket with Orchestrator;
  - Investigate Ticket;
  - Implement Ticket;
  - Review Ticket.
- Use `.yoi/ticket.config.toml` indirectly through the launcher.
- Show clear diagnostics/notices when:
  - Ticket config is malformed;
  - backend root is unusable;
  - selected Profile is invalid or spawn fails;
  - required Ticket id/slug/context is missing.
- Keep actions explicit and user-triggered. Do not introduce automatic scheduling.
- Do not add a spawned-Pod panel in this ticket.
- Do not add a generic arbitrary-role launcher UI.
- Do not bypass Ticket Intake / Orchestrator Routing / Preflight / Multi-agent Workflow gates.
- Prefer command/actionbar integration first if that is less invasive than adding a new full screen.

## Candidate command surface

Exact names may change during implementation, but the MVP should be close to:

```text
:ticket intake <ticket-or-new-context>
:ticket route <ticket-id-or-slug>
:ticket investigate <ticket-id-or-slug>
:ticket implement <ticket-id-or-slug>
:ticket review <ticket-id-or-slug>
```

If current TUI command parsing makes nested subcommands awkward, use a simpler MVP shape such as:

```text
:ticket-intake ...
:ticket-route ...
:ticket-investigate ...
:ticket-implement ...
:ticket-review ...
```

## Non-goals

- Implementing the role launcher.
- TUI spawned child Pod panel.
- Multi-Pod dashboard redesign.
- Scheduler/lease/queue automation.
- Stateful workflow engine.
- TicketUpdate tool.
- Worktree creation automation beyond what the launcher/Multi-agent Workflow already describes.
- Generic Role registry/UI.
- Arbitrary filesystem Ticket edits.

## Acceptance criteria

- TUI has user-triggered Ticket role actions/commands for the fixed roles or a clearly documented MVP subset.
- Actions call the shared launcher rather than building spawn requests directly.
- Role action failures surface as actionbar/command diagnostics without crashing TUI.
- The command/action help text makes clear what each role does.
- TUI tests cover parsing/execution path for actions and at least one failure diagnostic.
- `cargo test -p tui` or focused TUI tests pass.
- `cargo test -p client` passes if the launcher crate is touched by integration.
- `cargo check --workspace --all-targets`, `cargo fmt --check`, `git diff --check`, and `./tickets.sh doctor` pass.

## Dependencies

- Requires `ticket-role-pod-launcher`.
