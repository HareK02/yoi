---
id: '20260608-014602-remove-non-goals-from-workflow-templates'
slug: 'remove-non-goals-from-workflow-templates'
title: 'Remove Non-goals from workflow templates'
status: 'closed'
kind: 'task'
priority: 'P2'
labels: ['workflow', 'prompt', 'ticket', 'orchestration', 'cleanup']
workflow_state: 'done'
created_at: '2026-06-08T01:46:02Z'
updated_at: '2026-06-08T02:23:16Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T01:48:07Z'
---

## Background

Current workflow templates repeatedly ask for `Non-goals` or `Non-goals / constraints`:

- `ticket-intake-workflow.md` asks `やらないことは何か。`
- Ticket draft templates include `Non-goals:`.
- IntentPacket templates include `Non-goals / constraints: 今回やらないこと、触ってはいけない場所。`
- Orchestrator evidence checks combine `Non-goals / invariants`.

This wording encourages agents to fill a broad exclusion list with unrelated, obvious, or generic non-actions. It also mixes distinct concepts:

- binding decisions / invariants;
- implementation latitude;
- scope/authority boundaries;
- escalation conditions;
- adjacent work that belongs to another Ticket.

The result is noisy Tickets and intent packets where obvious unrelated exclusions obscure the actual implementation boundary.

## Goal

Remove `Non-goals` as a workflow section and simplify intent boundaries around two concepts:

1. `Binding decisions / invariants`
2. `Implementation latitude`

## Requirements

- Remove `Non-goals` / `Non-goals / constraints` sections from workflow templates and intent packet examples.
- Remove broad prompts such as `やらないことは何か。` from Intake requirements gathering.
- Do not replace `Non-goals` with another broad optional exclusion bucket unless explicitly needed.
- Use `Binding decisions / invariants` for:
  - explicit human/Orchestrator/Ticket decisions;
  - authority/design boundaries;
  - constraints the coder/reviewer must respect;
  - places/concepts that must not be changed when they are truly relevant.
- Use `Implementation latitude` for:
  - local tactics the coder may decide through investigation;
  - file/helper/test organization flexibility;
  - bounded uncertainty that reviewer can judge against intent and invariants.
- If adjacent excluded work must be mentioned, express it as a concrete binding decision or escalation condition rather than a generic `Non-goals` list.
- Update affected workflows:
  - `.yoi/workflow/ticket-intake-workflow.md`
  - `.yoi/workflow/ticket-orchestrator-routing.md`
  - `.yoi/workflow/ticket-preflight-workflow.md`
  - `.yoi/workflow/multi-agent-workflow.md`
- Update any prompt tests or role prompt guidance that expects `Non-goals` in intent packets.
- Keep reviewer guidance focused on recorded intent, binding decisions/invariants, implementation latitude, acceptance criteria, and explicit escalation conditions.

## Non-requirements

- Do not redesign the Ticket state machine in this ticket.
- Do not remove escalation conditions or validation sections.
- Do not remove the ability to state a concrete exclusion when it is genuinely a binding decision.

## Acceptance criteria

- Workflow templates no longer include a `Non-goals` / `Non-goals / constraints` section.
- IntentPacket examples use `Binding decisions / invariants` and `Implementation latitude` to express boundaries.
- Intake guidance no longer asks agents to enumerate generic `やらないこと`.
- Generated Tickets/intent packets are less likely to include unrelated obvious exclusions.
- Tests/docs/prompts that reference the old section are updated.
- `target/debug/yoi ticket doctor`, focused prompt/workflow tests if any, `cargo fmt --check`, and `git diff --check` pass.
