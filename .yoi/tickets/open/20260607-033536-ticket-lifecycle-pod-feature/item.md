---
id: 20260607-033536-ticket-lifecycle-pod-feature
slug: ticket-lifecycle-pod-feature
title: Ticket lifecycle pod feature
status: open
kind: task
priority: P1
labels: [ticket, pod-feature, tools, orchestration, workflow]
workflow_state: ready
created_at: 2026-06-07T03:35:36Z
updated_at: 2026-06-07T03:48:50Z
assignee: null
legacy_ticket: null
---

## Background

Panel/Orchestrator queue automation needs Ticket lifecycle operations to be available to Pods through the feature/tool system. This should not be modeled as an "Orchestrator feature": features should represent domain capabilities, not roles. Orchestrator, Intake, Companion, coder, and reviewer profiles may each be granted different subsets of the Ticket domain capability by policy/profile configuration.

The immediate motivation is queued Ticket handling: when a Ticket enters `workflow_state = queued`, the Orchestrator should be able to inspect Ticket state, record decisions/progress/blockers, and transition lifecycle state according to the Ticket workflow contract before starting worktree/Pod side effects. Prompt instructions are acceptable for sequencing initially; the first step is to make the Ticket lifecycle capability a proper `pod::feature` domain feature.

## Goal

Implement a `pod::feature`-based Ticket lifecycle domain feature that registers typed Ticket lifecycle tools for Pods, without making the feature role-specific.

## Domain model

- Feature identity should be Ticket-domain oriented, e.g. `ticket_lifecycle` / `ticket` rather than `orchestrator`.
- Role/profile policy decides which tools or permission level a Pod receives.
- Orchestrator may receive mutating lifecycle tools.
- Companion should receive read/status-only Ticket tools by default.
- Intake may receive create/update/intake-ready tools needed to materialize Tickets.
- Coder/reviewer may receive report/review/comment tools as appropriate.

## Requirements

- Add or extend a `pod::feature` module for Ticket lifecycle/tool registration.
- Register typed Ticket tools through the feature system rather than ad-hoc role-specific wiring.
- Cover at least the lifecycle operations needed by Panel/Orchestrator automation:
  - Ticket list/show read access;
  - append comment/decision/implementation report events;
  - append review events where granted;
  - `intake -> ready` intake-ready operation where granted;
  - `queued -> inprogress` and `inprogress -> done` workflow-state transitions where granted;
  - close Ticket where granted.
- Keep transition enforcement in the Ticket backend/tool implementation, not only in prompts.
- Do not make this an Orchestrator-only feature. The same domain feature must be grantable with different permissions to different role profiles.
- Provide a clear policy/config surface for read-only vs mutating Ticket lifecycle access, or use the existing feature/tool permission machinery if it already supports this distinction.
- Ensure tool descriptions/prompts communicate the sequencing contract:
  - queued Ticket implementation side effects should occur only after `queued -> inprogress` acceptance;
  - prompt sequencing is acceptable initially, but tool behavior must still enforce valid workflow transitions.
- Preserve current typed Ticket backend invariants and doctor validation.
- Keep local Pod/session assignment out of git-tracked Ticket metadata; use the existing local role session registry for local runtime association when needed.

## Non-goals

- Implementing full Orchestrator queue automation in this ticket.
- Implementing workflow state-machine-based dynamic tool gating.
- Replacing prompt/workflow sequencing with a complete operation-state enforcement runtime.
- Making `StartTicketWork` / `AcceptQueuedTicket` a composite tool unless it naturally falls out as a small wrapper; composite orchestration can remain a follow-up.
- Adding Companion Bash or broad workspace mutation authority.

## Acceptance criteria

- Ticket lifecycle tools are contributed through a Ticket-domain `pod::feature`.
- The feature can be granted to different roles/profiles with different authority levels; it is not hard-coded as an Orchestrator feature.
- Orchestrator-capable profiles can receive the Ticket lifecycle tools needed to inspect queued Tickets and perform `queued -> inprogress` / `inprogress -> done` transitions.
- Read-only/status roles can receive Ticket read tools without mutating lifecycle authority.
- Existing Ticket transition constraints remain enforced by typed backend/tool paths.
- Tests cover feature registration and at least one read-only vs mutating permission/grant distinction, or document why the existing feature permission tests already cover it.
