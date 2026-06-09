---
id: '20260608-072732-typed-ticket-relation-metadata'
slug: 'typed-ticket-relation-metadata'
title: 'Add non-hierarchical typed Ticket relations'
status: 'open'
kind: 'task'
priority: 'P1'
labels: ['ticket', 'relations', 'planning', 'panel', 'orchestrator']
workflow_state: 'ready'
created_at: '2026-06-08T07:27:32Z'
updated_at: '2026-06-09T01:02:17Z'
assignee: null
---

## Background

Ticket orchestration needs two distinct kinds of cross-Ticket coordination:

1. Project-level Ticket relations that are intrinsic to the Tickets themselves:
   - this feature depends on that Ticket;
   - this problem must be resolved before that Ticket is meaningful;
   - this Ticket blocks another Ticket;
   - this Ticket is related to, supersedes, or duplicates another Ticket.

2. Orchestrator execution-plan decisions:
   - these two Tickets are conceptually independent but touch conflicting code paths, so do not run them in parallel;
   - start Ticket A before B for the current batch;
   - wait for current reviewer/coder capacity;
   - this Ticket is planned queued but not yet accepted.

The existing `ticket-orchestration-plan-tool` belongs to the second category. It may be durable, but it is still about Orchestrator execution planning and local/current workset coordination.

This Ticket covers the first category: typed, durable Ticket-to-Ticket relations that can be shown and validated before the Orchestrator runs.

## Goal

Add typed Ticket relation metadata for project-level dependencies, blockers, related links, superseding links, and duplication links, stored with the Ticket system and visible through CLI/Panel/Ticket APIs independently of Orchestrator runtime plans. Do not model hierarchy, umbrella, parent/child, sub-ticket, or part-of relations.

## Requirements

- Provide a typed way to record Ticket-to-Ticket relations as project records.
- Support at least these non-hierarchical relation kinds, with final naming to be decided during implementation:
  - `depends_on`: this Ticket requires another Ticket to be completed/resolved first;
  - `blocks`: this Ticket blocks another Ticket;
  - `related`: non-blocking relation worth showing;
  - `supersedes` / `superseded_by` or `duplicate_of`: replacement/duplication relationship.
- Do not support hierarchy/container relation kinds in the first version:
  - no `parent` / `child`;
  - no `sub-ticket`;
  - no `umbrella`;
  - no `part_of` / `contains` decomposition semantics.
- Decide whether inverse non-hierarchical relations such as `blocked_by` and `superseded_by` are stored explicitly or derived from forward relations.
- Store relations in the Ticket backend in a durable, git-trackable form.
  - Prefer a typed field/artifact/event model that can be validated by the Ticket backend.
  - Avoid relying only on freeform Markdown thread text for mechanical relationships.
- Make relations visible before Orchestrator execution:
  - `TicketShow` should display relevant relations;
  - `TicketList` or Panel summaries should show at least blocking/dependency hints where space allows;
  - Panel/CLI should be able to indicate that a Ticket is blocked by unresolved dependencies.
- Add validation:
  - `ticket doctor` should detect dangling Ticket references;
  - invalid relation kinds should fail validation;
  - self-dependencies and obvious invalid cycles should be rejected or diagnosed;
  - closed/resolved dependencies should be distinguishable from open blockers.
- Integrate with Intake/Planning:
  - Intake/Planning should be able to record known project-level relations when materializing/refining a Ticket;
  - dependency/blocking/related/supersedes/duplicate relations should be available for Ticket shaping and review;
  - Intake/Planning must not create parent/child or umbrella-style relations as a replacement for umbrella Tickets.
- Integrate with Orchestrator routing as input, not as runtime plan state:
  - Orchestrator should consult Ticket relations before accepting queued work;
  - unresolved `depends_on` / blocking relations should prevent acceptance or return the Ticket to planning/blocked with a visible reason;
  - OrchestrationPlan may use Ticket relations as constraints, but should not own or invent project-level dependency metadata.
- Keep this distinct from local execution state:
  - do not store Pod/session claims, worktree paths, active branch ownership, or reviewer/coder assignments in Ticket relation metadata;
  - those remain in the local role session registry / OrchestrationPlan / Pod metadata as appropriate.

## Non-goals

- Replacing OrchestrationPlan execution decisions such as current capacity, do-not-parallelize because of code conflicts, or planned queued order for the current batch.
- Implementing a full dependency graph scheduler.
- Making every related Ticket relation block queuing automatically.
- Encoding transient local Pod/worktree claims in git-tracked Ticket metadata.
- Representing Ticket hierarchy, umbrella containers, parent/child decomposition, sub-tickets, or part-of grouping.

## Acceptance criteria

- Project-level Ticket relations can be recorded in a typed durable form.
- `TicketShow` displays relation metadata.
- `ticket doctor` validates dangling references, invalid kinds, self-relations, and at least obvious invalid dependency cycles or reports bounded diagnostics.
- Panel or Ticket list can surface dependency/blocking hints before the Orchestrator runs.
- Intake/Planning workflow guidance explains when to create project-level relations.
- Orchestrator routing guidance treats these relations as input constraints distinct from OrchestrationPlan runtime decisions.
- Documentation clearly distinguishes:
  - `depends_on` / `blocks` as non-hierarchical Ticket relation metadata;
  - `related` / `supersedes` / `duplicate_of` as non-blocking or replacement metadata;
  - unsupported hierarchy/container semantics such as parent/child, umbrella, sub-ticket, and part-of;
  - `do_not_parallelize` / capacity / current planned order as OrchestrationPlan execution decisions;
  - Pod/session/worktree ownership as local runtime claims.
- Focused tests, `target/debug/yoi ticket doctor`, `cargo fmt --check`, and `git diff --check` pass.
