---
id: 20260607-230044-relax-implementation-planning-readiness
slug: relax-implementation-planning-readiness
title: Relax implementation planning readiness toward intent and reviewability
status: open
kind: task
priority: P2
labels: [workflow, ticket, orchestrator, planning, review]
workflow_state: done
created_at: 2026-06-07T23:00:44Z
updated_at: 2026-06-07T23:38:32Z
assignee: null
queued_by: workspace-panel
queued_at: 2026-06-07T23:03:35Z
---

## Background

Current Intake / Planning / Orchestrator workflow wording tends to require implementation strategy to be narrowed too early. In practice, the most reliable implementation investigation often happens when the implementation Pod/coder reads the code and attempts the change in the scoped worktree.

The higher-level planning/intake stage should prioritize clarifying intent, constraints, and reviewable outcomes rather than over-specifying the implementation approach. Reviewers need enough intent and acceptance criteria to judge whether the implementation matches the user's/project's intent; coders should retain room to discover the best local implementation path unless a design decision is explicitly fixed.

Current problem:

- Intake/Planning `ready` and Orchestrator `implementation_ready` are easy to interpret as requiring a nearly fixed implementation plan.
- Orchestrator may over-return Tickets to planning when some uncertainty could safely be resolved by the coder during implementation and checked by reviewer.
- This creates extra planning-return churn and can make the workflow feel stalled.

Desired direction:

- Clarify intent first.
- Keep implementation judgment available to the coder where safe.
- Make reviewer validation against intent the key quality gate.
- Treat explicit design decisions/invariants as binding when present.
- Use return-to-planning for genuine design-boundary/product/API/authority uncertainty, not every implementation unknown.

## Goal

Adjust Ticket Intake/Planning, Orchestrator routing, and Planning workflow guidance so readiness is based on clear intent and reviewability, while preserving coder discretion for implementation details unless constrained by explicit decisions.

## Requirements

### Readiness semantics

- Define `ready` as ready for Orchestrator routing, not necessarily fully implementation-planned.
- Define implementation handoff readiness as:
  - user/project intent is clear;
  - observable acceptance criteria are clear enough;
  - important constraints/invariants/non-goals are captured;
  - reviewer can judge whether the resulting implementation satisfies the intent;
  - known binding design decisions are recorded;
  - unresolved questions are either non-blocking implementation details or explicitly listed as escalation conditions.
- Avoid requiring Intake/Planning to pre-decide local implementation tactics when they can be safely discovered by the coder.

### Orchestrator routing

- Narrow return-to-planning so it is reserved for real design/product/API/authority-boundary uncertainty, not ordinary code investigation or implementation tactic selection.
- Make `implementation_ready` allow bounded implementation uncertainty when:
  - intent and acceptance criteria are clear;
  - coder can investigate safely within scope;
  - reviewer can evaluate against recorded intent;
  - escalation conditions are defined.
- Keep return-to-planning for cases where implementing would implicitly decide product/API/UX/authority boundaries or violate known design constraints.
- Require Orchestrator to record whether implementation latitude is allowed and what must be escalated.

### Coder/reviewer handoff

- Update intent packet guidance to distinguish:
  - Binding decisions / invariants the coder must follow.
  - Implementation latitude the coder may decide through code investigation.
  - Escalation conditions that require returning to Orchestrator/human.
- Reviewer instructions should emphasize judging the implementation against intent, constraints, and acceptance criteria, not whether it followed an unrecorded preferred approach.
- If a design approach is explicitly stated in the Ticket/thread, reviewer should verify the implementation follows it or records a justified deviation.

### Workflow/docs updates

- Update `.yoi/workflow/ticket-intake-workflow.md`.
- Update `.yoi/workflow/ticket-orchestrator-routing.md`.
- Update the planning/requirements-sync workflow guidance.
- Update `.yoi/workflow/multi-agent-workflow.md` if needed to reflect coder latitude and reviewer intent-checking.
- Adjust role prompt wording/tests if generated prompt guidance currently over-demands pre-decided implementation plans.

## Non-goals

- Do not remove return-to-planning; narrow when it is required.
- Do not allow coders to make product/API/authority decisions silently.
- Do not weaken explicit design decisions or invariants recorded by humans/Orchestrator.
- Do not change Ticket state machine names in this ticket unless it is already being handled by the planning-state rename work.

## Acceptance criteria

- Workflows clearly distinguish ready-for-routing, implementation handoff, and fully specified implementation plan.
- Orchestrator routing guidance no longer treats ordinary implementation investigation as automatic return-to-planning.
- Intent packet template includes sections for binding decisions, implementation latitude, and escalation conditions.
- Reviewer guidance focuses on intent/constraints/acceptance criteria and explicit decisions.
- Example wording makes clear that coder investigation is acceptable when intent is clear and risks are bounded.
- Existing Ticket planning/routing tests or prompt tests are updated if affected.
- `target/debug/yoi ticket doctor`, `cargo fmt --check`, and `git diff --check` pass.
