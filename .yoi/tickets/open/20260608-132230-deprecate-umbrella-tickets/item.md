---
id: '20260608-132230-deprecate-umbrella-tickets'
slug: 'deprecate-umbrella-tickets'
title: 'Deprecate umbrella Tickets'
status: 'open'
kind: 'task'
priority: 'P2'
labels: ['ticket', 'workflow', 'documentation', 'planning']
workflow_state: 'inprogress'
created_at: '2026-06-08T13:22:30Z'
updated_at: '2026-06-09T01:20:28Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-09T01:14:14Z'
---

## Background

Umbrella Tickets have been used as broad containers that split into several implementation Tickets. In practice this creates ambiguous lifecycle semantics:

- it is unclear when the umbrella itself should be closed;
- child Tickets may be complete while the umbrella remains open;
- AI-created splits require the user to re-open the umbrella and inspect whether all child work is truly sufficient;
- the umbrella can become a mix of initial request, progress container, split decision log, Objective-like strategy context, and completion proxy.

The `workspace-panel-orchestrator-queue-automation` Ticket exposed this problem: core child slices were completed, but the umbrella remained open and its close condition was not obvious.

The emerging model is:

- Ticket: implementable/completable work item shaped by Intake/Planning.
- Objective: medium-term goal, motivation, strategy, and success criteria.
- Typed Ticket relations: non-hierarchical dependency/related/supersedes/blocking metadata between concrete Tickets.
- OrchestrationPlan: current execution planning for queued/in-progress work.

Umbrella Tickets do not fit cleanly into that model.

## Goal

Deprecate the use of umbrella Tickets as broad multi-Ticket containers, and update workflow/docs so future work is represented by concrete Tickets plus Objectives/relations instead.

## Requirements

- Document that new umbrella Tickets should not be created for broad multi-Ticket efforts.
- State that Tickets should be concrete work items that can be implemented, reviewed, validated, and closed on their own terms.
- When a broad request must be split:
  - create concrete implementable Tickets for the slices;
  - record the split decision in the relevant Ticket/Objective/thread context;
  - use an Objective for medium-term goal/strategy context when needed;
  - use typed Ticket relations, once available, only for non-hierarchical dependency/related/blocking/replacement metadata;
  - do not create parent/child, sub-ticket, umbrella, part-of, or other hierarchy/container relations as a replacement for umbrella Tickets;
  - do not keep a separate umbrella Ticket open merely as a progress container.
- Clarify the migration/cleanup path for existing umbrella Tickets.
  - They may be closed as superseded/decomposed once concrete follow-up Tickets/Objective context exist.
  - Closing an umbrella in this way does not mean every related future concern is complete; it means the umbrella container role is retired.
  - The close resolution should list completed child/concrete Tickets and remaining follow-up Tickets/Objectives.
- Update workflow guidance for Intake/Planning and Orchestrator so they avoid creating umbrella Tickets or parent/child sub-ticket hierarchies during split/refinement.
- Ensure this does not block creating an initial planning Ticket when a user explicitly asks for a concrete design/investigation work item; the deprecated pattern is the long-lived umbrella progress container.

## Non-goals

- Designing typed Ticket relation metadata; keep relation implementation details in `typed-ticket-relation-metadata`, but this Ticket fixes the policy that relation metadata must not reintroduce hierarchy/container semantics.
- Implementing Objective records; covered by `objective-records-for-medium-term-goals`.
- Rewriting every historical umbrella immediately.
- Removing thread history from existing umbrella Tickets.

## Acceptance criteria

- Development/workflow docs explain that umbrella Tickets are deprecated and should not be created for new broad efforts.
- Development/workflow docs also state that parent/child, sub-ticket, part-of, or umbrella-style hierarchy relations should not be used as substitutes.
- Intake/Planning guidance explains how to split broad requests without creating an umbrella progress container or sub-ticket hierarchy.
- Orchestrator guidance explains how to close or retire existing umbrella Tickets once concrete Tickets/Objectives carry the remaining work.
- At least `workspace-panel-orchestrator-queue-automation` has a clear migration/close recommendation recorded or is handled in a follow-up.
- `target/debug/yoi ticket doctor` and `git diff --check` pass.
