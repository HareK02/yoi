---
id: '20260608-071722-orchestrator-return-to-planning-context-policy'
slug: 'orchestrator-return-to-planning-context-policy'
title: 'Require project context before Orchestrator returns Tickets to planning'
status: 'open'
kind: 'task'
priority: 'P1'
labels: ['ticket', 'orchestrator', 'planning', 'workflow', 'prompt']
workflow_state: 'ready'
created_at: '2026-06-08T07:17:22Z'
updated_at: '2026-06-08T10:58:29Z'
assignee: null
---

## Background

`replace-intake-state-with-planning` represents implementation-readiness gaps as a return to the `planning` lane with a visible reason.

That creates an important follow-up requirement for Orchestrator routing: returning a Ticket to planning must not become the new form of blind planning rejection. The Orchestrator should not look only at Ticket text, `risk_flags`, or broad risky domains and decide to stop. It must consider relevant project context before deciding that implementation cannot proceed.

The recent `allow-spawnpod-child-workspace-cwd` discussion exposed the failure mode: a Ticket can touch authority/Pod/scope areas while still containing enough binding decisions and acceptance criteria to route to implementation. In that case, risk should become reviewer focus and escalation conditions, not an automatic return to planning.

## Goal

Update Orchestrator routing guidance so a Ticket is returned to planning only when the Orchestrator can identify concrete missing information or binding decisions after checking enough project context.

## Requirements

- Treat this as a follow-up to `replace-intake-state-with-planning`.
  - First settle the planning-state model.
  - Then apply this policy to the Orchestrator's return-to-planning decision.
- Update `ticket-orchestrator-routing` or equivalent prompt/workflow guidance.
- Before returning a queued/ready Ticket to planning, the Orchestrator must inspect enough context to distinguish:
  - a genuinely missing binding decision, requirement, invariant, or acceptance criterion;
  - an existing recorded decision in the Ticket thread, related Tickets, docs, workflows, code, or durable project context;
  - a bounded local implementation tactic that a coder can resolve during implementation.
- Risk flags are context lookup triggers and reviewer focus, not stop gates.
  - `authority-boundary`, `scope`, `permission`, `Pod`, `persistence`, `prompt-context`, `public-api`, and similar risk areas should cause targeted context checking.
  - They should not by themselves justify returning to planning.
- If the Orchestrator returns a Ticket to planning, it must record:
  - the concrete missing decision/information;
  - the context it checked;
  - why the gap cannot be safely handled as implementation latitude;
  - the next planning question/action.
- If the Orchestrator cannot name a concrete missing decision/information item after context checking, it should not return the Ticket to planning merely because it is risky.
  - It should instead proceed as implementation-ready when other criteria are satisfied, with an IntentPacket that includes invariants, implementation latitude, escalation conditions, and reviewer focus.
- Keep the context check bounded and relevant.
  - Do not require broad repository audits for every Ticket.
  - Prefer targeted reads of Ticket thread/artifacts, related Tickets, known workflows/docs, current code map, git/worktree/Pod state, and relevant durable decisions.
- Ensure the routing decision template includes an `Evidence checked` section strong enough to reveal whether the Orchestrator actually considered project context.

## Non-goals

- Reintroducing a separate readiness-check operation, state, or lane outside planning.
- Making every queued Ticket require extensive code archaeology.
- Removing Orchestrator judgment; the point is to make the judgment evidence-based.
- Letting coder Pods silently decide product/API/authority boundaries that are genuinely missing.

## Acceptance criteria

- Orchestrator workflow/prompt guidance says Tickets must not be returned to planning from Ticket text or risk flags alone.
- Returning to planning requires a concrete missing decision/information item and an evidence record of relevant context checked.
- Risk flags are documented as lookup/reviewer-focus signals, not gates.
- Implementation-ready routing explicitly allows risky-but-specified Tickets to proceed with IntentPacket invariants, escalation conditions, and reviewer focus.
- The workflow includes or references an example equivalent to `allow-spawnpod-child-workspace-cwd`: authority-adjacent work with explicit binding decisions should proceed rather than be returned to planning solely because of risk category.
- Focused tests or prompt/workflow validation cover at least:
  - risky but specified Ticket -> implementation-ready;
  - genuinely missing decision -> return to planning;
  - risk flag alone is insufficient as the reason.
- `target/debug/yoi ticket doctor` and `git diff --check` pass.
