<!-- event: create author: yoi ticket at: 2026-06-05T21:07:03Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-05T21:09:19Z -->

## Plan

Plan: design the workspace orchestration panel before implementation.

The panel is not a `:ticket` command extension. It is a workspace-scoped orchestration surface with:

- Companion foreground management chat;
- Ticket Intake composer target that sends user input directly to Intake, not Companion history;
- background workspace Orchestrator restored/spawned as `<dir-name>-orchestrator`;
- Intake -> Orchestrator handoff;
- an action model prioritizing human-required Ticket/Intake decisions over raw Pod idle state.

Implementation is split into child tickets:

1. `workspace-orchestration-panel-design`
2. `workspace-panel-orchestrator-lifecycle`
3. `workspace-panel-composer-targets`
4. `ticket-intake-orchestrator-handoff`
5. `workspace-panel-action-model`

Existing `:ticket ...` commands remain as low-level fallback, not the main UX.


---
