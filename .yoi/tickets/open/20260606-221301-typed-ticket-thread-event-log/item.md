---
id: 20260606-221301-typed-ticket-thread-event-log
slug: typed-ticket-thread-event-log
title: Typed Ticket thread event log for workflow state changes
status: open
kind: task
priority: P1
labels: [ticket, orchestration, state, audit]
created_at: 2026-06-06T22:13:01Z
updated_at: 2026-06-06T22:14:29Z
assignee: null
legacy_ticket: null
---

## Background

The explicit workflow state design moves current Ticket workflow state into frontmatter, but frontmatter alone only records the current value. The system also needs a small append-only audit trail explaining why state changed and which actor changed it.

The current Ticket thread is too freeform to be a reliable workflow record. It tends to become a place for arbitrary comments and AI-generated prose, while still not clearly recording state transitions or Intake decisions.

## Goal

Make `thread.md` a concise typed event log for durable Ticket events, especially workflow state changes and Intake summaries, rather than a general conversation transcript.

## Authority split

- `item.md` frontmatter: current mechanical state authority.
- `item.md` body: current human-readable Ticket snapshot: background, goal, requirements, non-goals, acceptance criteria.
- `thread.md`: append-only typed event log explaining state transitions, important decisions, implementation reports, reviews, and close events.
- Pod/session logs: full conversational/runtime transcript. Do not copy full Pod conversations into Ticket thread.

## Event model

Add or formalize typed thread events for at least:

```text
create
state_changed
intake_summary
decision
implementation_report
review
close
```

`comment` may remain for manual escape hatches if needed, but Orchestrator/Intake/panel workflow should not rely on freeform comments as the primary record.

## State transition logging

Every workflow state mutation should append a concise `state_changed` event containing at least:

```text
from
to
actor
reason
summary or refs
```

State transitions should be performed through backend APIs that update frontmatter and append the thread event as one logical mutation. Avoid separate call sites that can update frontmatter without an event or append an event without updating current state.

## Intake logging

Intake should not dump the full Intake Pod conversation into `thread.md`. When Intake materializes a Ticket or marks it `ready`, it should write a concise `intake_summary` event containing:

- accepted user intent;
- important clarified decisions;
- requirements / acceptance criteria summary;
- known non-goals;
- unresolved questions, if any.

The Ticket body should hold the current snapshot. The thread should hold the reason/history for how that snapshot became accepted.

## Constraints

- Keep events concise and bounded.
- Do not persist hidden reasoning.
- Do not paste raw transcripts by default.
- Do not use thread events as the source of current workflow state; current state lives in frontmatter.
- Do not store transient Pod activity as Ticket state.
- Preserve existing historical thread records; do not mass-rewrite old history unless a migration explicitly requires it.

## Relationship to explicit workflow state

This ticket is a companion/prerequisite to `explicit-ticket-workflow-state`. That ticket defines the current-state fields and panel semantics. This ticket defines the audit/event-log mechanics so state transitions remain explainable without relying on noisy freeform thread prose.

## Non-goals

- Building a scheduler/lease system.
- Replacing Pod/session logs.
- Capturing full Intake conversations in Ticket records.
- Rewriting all historical threads.
- Changing public UI layout by itself.

## Acceptance criteria

- A typed event model exists for `state_changed` and `intake_summary` at minimum.
- Workflow state changes have an API path that updates current state and appends a `state_changed` event together.
- Intake-ready/materialization flow has a bounded `intake_summary` event path.
- Panel/Orchestrator state changes do not rely on freeform comments for auditability.
- Existing Ticket doctor/lint checks accept the new event types and reject malformed required fields where practical.
- Documentation explains the split between current state, current Ticket snapshot, append-only thread events, and Pod/session transcripts.
