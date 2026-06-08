---
id: 20260605-040104-ticket-intake-workflow
slug: ticket-intake-workflow
title: Ticket intake workflow
status: closed
kind: task
priority: P1
labels: [ticket, intake, workflow, orchestration]
created_at: 2026-06-05T04:01:04Z
updated_at: 2026-06-05T06:10:56Z
assignee: null
---

## Background

The multi-agent system needs an Intake role that turns a user's vague or broad request into an agreed Ticket before Orchestrator starts scheduling implementation work.

The Intake role is not a scheduler and does not spawn coder/reviewer Pods. It clarifies intent, checks for duplicates/related tickets, asks the user for missing requirements, and creates or updates Tickets after agreement.

This ticket depends on typed Ticket tools. Without a built-in Ticket tool surface, Intake would need arbitrary filesystem write scope or ad hoc shell access, which is not the desired authority boundary.

## Requirements

- Define an Intake workflow/profile/prompt for Ticket intake.
- Intake should be able to:
  - understand the user's requested change;
  - inspect existing Tickets for duplicates or related work;
  - inspect relevant repository files when needed and permitted;
  - ask clarifying questions;
  - propose Ticket title/kind/priority/labels;
  - summarize background, requirements, acceptance criteria, non-goals, escalation conditions, risk flags, and readiness;
  - create a Ticket after user agreement;
  - update an existing Ticket when the user asks to refine known work.
- Intake-created Tickets should be suitable for Orchestrator routing without a second full reinterpretation pass.
- Intake must classify readiness explicitly:
  - `implementation_ready`;
  - `requirements_sync_needed`;
  - `spike_needed`;
  - `blocked`;
  - or `unspecified` only when unavoidable.
- Intake should mark `needs_preflight` for tickets with design/authority/scope/history/prompt-context risk.
- Intake should avoid persisting private/secret-like material in Ticket bodies, thread entries, artifacts, or diagnostics.

## User agreement rule

Intake must not silently convert an unresolved draft into an official Ticket.

MVP behavior:

- Draft state can live in the conversation while clarifying.
- Intake creates a Ticket only after the user accepts the proposed contents or explicitly asks to create it.
- If the user wants to record unresolved work, Intake may create a Ticket with readiness such as `requirements_sync_needed` or `spike_needed`, but the unresolved state must be explicit in the Ticket.

## Non-goals

- Scheduling implementation.
- Spawning coder/reviewer Pods.
- Creating worktrees.
- Closing Tickets.
- Building a dedicated Inbox UI or storage.
- Building an unattended maintainer/scheduler.
- Replacing `multi-agent-workflow`.
- Implementing Ticket backend/tools.

## Acceptance criteria

- A user-invocable Intake workflow/profile exists and uses Ticket terminology consistently.
- Intake can create a Ticket through Ticket tools after user agreement.
- Intake can update/comment on an existing Ticket through Ticket tools when refining known work.
- Intake checks for duplicate/related Tickets before creating a new one when practical.
- Intake-created Tickets include enough information for Orchestrator to classify next action:
  - issue/background;
  - requirements;
  - acceptance criteria;
  - non-goals where relevant;
  - readiness;
  - needs-preflight;
  - risk flags;
  - escalation conditions where relevant.
- Intake does not require general repository write scope to create Tickets.
- Intake does not start implementation or spawn coder/reviewer Pods.
- Secret/private-context handling is documented in the workflow/prompt.
- Focused tests or prompt/resource linting cover the installed workflow/profile where applicable.
- `cargo check --workspace --all-targets`, `cargo fmt --check`, `git diff --check`, and `./tickets.sh doctor` pass if code changes are included.

## Dependencies

- Requires `ticket-local-files-backend`.
- Requires `ticket-built-in-feature-tools` for the intended authority boundary.

## Follow-up tickets

- `ticket-orchestrator-routing`
