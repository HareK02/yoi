---
id: 20260605-040104-ticket-orchestrator-routing
slug: ticket-orchestrator-routing
title: Ticket orchestrator routing
status: open
kind: task
priority: P1
labels: [ticket, orchestrator, routing, orchestration]
created_at: 2026-06-05T04:01:04Z
updated_at: 2026-06-05T06:41:34Z
assignee: null
legacy_ticket: null
---

## Background

Once Tickets can be created by Intake and read/mutated through built-in Ticket tools, Orchestrator needs a routing layer that decides what should happen next for each Ticket.

This is not an unattended scheduler MVP. The goal is to make Orchestrator's next-action classification explicit and tool/workflow-backed, so multi-agent operation can move from ad hoc conversation into repeatable routing decisions.

## Requirements

- Define Orchestrator routing states for Tickets.
- Route Tickets into next actions such as:
  - requirements sync / return to Intake or human;
  - preflight;
  - read-only spike/investigation;
  - implementation-ready delegation;
  - review loop;
  - blocked/action-required;
  - close/complete;
  - defer/pending.
- Use Ticket fields/events rather than hidden conversation state as routing authority.
- Produce an intent packet for implementation-ready Tickets that can be handed to `multi-agent-workflow`.
- Preserve current manual merge/close authority unless explicitly authorized otherwise.
- Keep final design-boundary decisions, merge, cleanup, and Ticket close under Orchestrator/human control.
- Avoid automatic scheduler/lease behavior in MVP.

## Candidate routing classification

```text
implementation_ready:
- Ticket has requirements, acceptance criteria, non-goals/invariants where needed, and validation guidance.
- Orchestrator may create worktree + coder/reviewer assignment through existing multi-agent workflow.

preflight_needed:
- Ticket touches design/authority/scope/history/prompt-context or has multiple plausible implementation strategies.
- Orchestrator should run ticket-preflight-workflow before implementation.

requirements_sync_needed:
- Ticket purpose is visible but user-facing behavior, terms, scope, or acceptance criteria are unclear.
- Return to Intake/human.

spike_needed:
- Technical feasibility, dependency/licensing/performance/diagnostic behavior, or current-code mapping must be investigated first.
- Spawn read-only investigation if authorized.

review_needed:
- Implementation report/diff exists and external review has not approved.

blocked_action_required:
- Human decision or external event is required.

closed_or_noop:
- No routing action.
```

## Integration points

- Existing `.yoi/workflow/multi-agent-workflow.md` remains the implementation/review delegation workflow.
- Existing `.yoi/workflow/ticket-preflight-workflow.md` remains the preflight workflow.
- Ticket tools provide list/show/comment/update operations.
- Future action-required UI/dashboard state can consume the routing result, but UI work is not required in this ticket.

## Non-goals

- Building an unattended scheduler.
- LeaseStore / queue persistence.
- Automatically spawning Pods without human/orchestrator authorization.
- Implementing Ticket backend/tools.
- Implementing Intake workflow.
- Building TUI spawned-Pod panel.
- Changing merge/close policy.
- Replacing `multi-agent-workflow`.

## Acceptance criteria

- Orchestrator has a documented and testable routing classification over Tickets.
- Routing decisions are based on Ticket fields/events and current repository/Pod state that is explicitly inspected.
- Orchestrator can produce a concise intent packet for implementation-ready Tickets.
- Orchestrator can record routing decisions/comments back to the Ticket.
- Preflight-needed Tickets are not blindly delegated to coder Pods.
- Requirements-sync/spike/blocked classifications produce clear next questions or follow-up actions.
- Existing multi-agent workflow remains the execution path for coder/reviewer delegation.
- No automatic scheduler/lease system is introduced.
- Focused tests/docs/workflow updates cover classification behavior where applicable.
- `cargo check --workspace --all-targets`, `cargo fmt --check`, `git diff --check`, and `./tickets.sh doctor` pass if code changes are included.

## Dependencies

- Requires `ticket-local-files-backend`.
- Requires `ticket-built-in-feature-tools` for tool-backed routing updates.
- Benefits from `ticket-intake-workflow`, but can be preflighted in parallel once Ticket fields are stable.
