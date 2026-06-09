---
title: "Define Objective records for medium-term goals"
state: "ready"
created_at: "2026-06-08T12:54:30Z"
updated_at: "2026-06-09T00:17:13Z"
---

## Background

Tickets are settling into a concrete implementation/work-item unit: Intake/Planning shapes user intent into Tickets that can be implemented, reviewed, and completed. Parent/child Ticket relationships are useful for decomposition, but they are not sufficient for medium-term strategic context.

Some work needs a separate record for the goal, motivation, strategy, success criteria, and decision context that spans multiple Tickets without turning the Ticket hierarchy into a roadmap/project-management system.

Use the term `Objective` for this concept. `Project` is too broad, and `Initiative` feels too business/process-heavy.

## Goal

Design a lightweight `Objective` record type for medium-term goals that can group and contextualize multiple Tickets while keeping Tickets focused on implementable work items.

## Requirements

- Define `Objective` as a first-class project record distinct from Ticket.
  - Objective: medium-term goal, motivation, strategy, success criteria, and decision context.
  - Ticket: implementable work item shaped by Intake/Planning.
  - OrchestrationPlan: current execution plan for queued/in-progress work.
- Keep Objectives lightweight and Markdown-oriented initially.
  - Avoid building a full roadmap/project-management system in the first version.
  - Prefer a simple local file backend under a clear `.yoi/` path.
- An Objective should be able to record at least:
  - title;
  - goal statement;
  - motivation/background;
  - strategy or design direction;
  - success criteria / exit conditions;
  - linked Tickets;
  - important decisions or pivots;
  - current state such as active/paused/done, if useful.
- Tickets should be able to reference an Objective or be listed under it.
  - The relation should not replace Ticket dependency/blocking metadata.
  - A Ticket can belong to an Objective without depending on every other Ticket in that Objective.
- Orchestrator/Intake/Planning should be able to use Objective context when shaping or routing Tickets.
  - Objective context should help answer “why this direction?” and “what judgment criteria apply?”
  - It should not allow agents to skip reading the actual Ticket body/thread before implementation decisions.
- Panel/CLI should eventually be able to show Objective context and linked Tickets, but first design can focus on record shape and workflow semantics.
- Clarify how Objectives differ from:
  - parent/child Tickets;
  - typed Ticket relations such as `depends_on` / `blocks`;
  - OrchestrationPlan execution decisions;
  - local Pod/session runtime claims.

## Non-goals

- Replacing Tickets.
- Replacing typed Ticket relation metadata.
- Replacing OrchestrationPlan.
- Implementing full roadmap scheduling, milestones, OKRs, or dependency graph solving.
- Making every Ticket require an Objective.

## Acceptance criteria

- There is a documented concept and proposed file/API shape for `Objective` records.
- The design explains when to create an Objective versus a parent Ticket.
- The design explains how Tickets link to Objectives without conflating that link with dependency/blocking relations.
- The design explains how Orchestrator/Intake should consult Objective context while still treating Ticket body/thread as the implementation authority.
- A first implementation path is identified with minimal schema and tooling scope.
- `target/debug/yoi ticket doctor` and `git diff --check` pass for any record changes.
