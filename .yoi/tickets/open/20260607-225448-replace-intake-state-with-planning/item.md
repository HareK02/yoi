---
id: 20260607-225448-replace-intake-state-with-planning
slug: replace-intake-state-with-planning
title: Replace Ticket intake state with planning state
status: open
kind: task
priority: P1
labels: [ticket, workflow-state, orchestrator, panel, intake, planning]
workflow_state: 'inprogress'
created_at: 2026-06-07T22:54:48Z
updated_at: '2026-06-08T09:14:49Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T08:35:07Z'
---

## Background

The Ticket workflow state `intake` is currently used for more than initial intake. It also represents clarification, requirements synchronization, and preflight/planning work before a Ticket is implementation-ready.

This naming is misleading in panel/orchestration flows:

- A Ticket can be returned from Orchestrator routing because implementation requires preflight or requirements clarification.
- Such a Ticket should go back to the planning/clarification lane, not appear as a brand-new intake item.
- Current Orchestrator `preflight_needed` behavior can look like nothing happened because it may only comment and avoid starting coder Pods.
- Users need a visible state/action that says the Ticket is under planning / needs planning work.

Desired direction:

- Remove `intake` as the user-facing workflow state name.
- Replace it with a planning-oriented state such as `planning` or `under_planning`.
- Orchestrator `preflight_needed` / requirements-sync-needed routing should return the Ticket to this planning state and notify any active Intake/Planning Pod if one exists.

## Goal

Rename/reframe Ticket `workflow_state = intake` into a planning-oriented state and make Orchestrator preflight-needed routing visibly return Tickets to that lane instead of silently stalling.

## Requirements

### State model / migration

- Replace `intake` in the Ticket workflow-state model with a planning-oriented state name.
  - Candidate names: `planning`, `under_planning`, or another concise name chosen during implementation.
  - Prefer a name that communicates requirements clarification / preflight preparation, not only initial intake.
- Define migration behavior for existing Tickets with `workflow_state: intake`.
  - Either migrate records to the new value or support legacy `intake` as an input alias with normalized output.
- Update typed transition graph accordingly.
  - Existing start path becomes `planning -> ready -> queued -> inprogress -> done`.
  - Allow Orchestrator to route `queued` or `ready` back to `planning` when preflight/requirements sync is needed, with a reason.
- Update CLI, Ticket tools, TUI panel, prompts, workflow docs, and tests that refer to `intake` as a workflow state.
- Keep the Intake role name separate if still useful for the Pod/persona; this ticket is about workflow state naming and lane semantics.

### Orchestrator preflight-needed behavior

- When Orchestrator determines `preflight_needed` or requirements-sync-needed:
  - Do not start coder Pods.
  - Record a typed routing/state-change event with a clear reason.
  - Return the Ticket to the planning state, or otherwise ensure the planning lane visibly owns it.
  - Surface the reason in Panel/Ticket UI so the user can see why implementation did not start.
- If a live/restorable Intake/Planning Pod claim exists for that Ticket, notify/send it the routing-back reason and requested planning/preflight action.
- If no such Pod exists, Panel should show that planning/clarification/preflight attention is needed and provide the existing launch path.
- Preserve auditability: the state transition and reason must be recorded in Ticket files/thread.

### UI / terminology

- Replace user-facing `intake` workflow-state labels in Panel and docs with the chosen planning-oriented label.
- Clarify that this lane covers:
  - initial Ticket clarification;
  - requirements synchronization;
  - preflight preparation;
  - Orchestrator return-to-planning decisions.
- Avoid confusing `Intake` role/profile naming with the workflow state if the role remains named Intake.

## Non-goals

- Do not redesign the entire Ticket workflow state machine beyond replacing/reframing `intake` and adding return-to-planning behavior.
- Do not remove the Intake Pod role/profile unless a separate decision is made.
- Do not implement arbitrary role registries.
- Do not change implementation worktree/coder/reviewer mechanics except where Orchestrator routing needs to return to planning.

## Acceptance criteria

- New/updated Tickets use the planning-oriented workflow-state name instead of `intake`.
- Existing `workflow_state: intake` Tickets are migrated or parsed compatibly and emitted in the new form according to the chosen policy.
- Panel no longer presents the state as simply `intake`; it shows planning/clarification/preflight terminology.
- Orchestrator `preflight_needed` routing produces a visible state/typed event and returns the Ticket to the planning lane.
- A claimed live/restorable Intake/Planning Pod is notified when Orchestrator returns a Ticket for planning/preflight.
- Without a claimed Pod, Panel clearly shows planning/preflight attention is needed and provides a launch path.
- Tests cover state parsing/migration, transition graph, Panel action derivation, and Orchestrator return-to-planning behavior.
- Relevant docs/workflows are updated.
- `target/debug/yoi ticket doctor`, focused `cargo test` suites, `cargo fmt --check`, and `git diff --check` pass.
