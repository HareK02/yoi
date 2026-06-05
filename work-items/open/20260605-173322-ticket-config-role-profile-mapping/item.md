---
id: 20260605-173322-ticket-config-role-profile-mapping
slug: ticket-config-role-profile-mapping
title: Ticket config role profile mapping
status: open
kind: task
priority: P1
labels: [ticket, config, profile, orchestration]
created_at: 2026-06-05T17:33:22Z
updated_at: 2026-06-05T17:35:08Z
assignee: null
legacy_ticket: null
---

## Background

Ticket orchestration now has typed Ticket backend/tools and workflows for Intake and Orchestrator routing. The next step before TUI role actions is to make the workspace's Ticket orchestration configuration explicit.

The project should not introduce an arbitrary Role registry. The roles needed here are fixed by the Ticket feature/workflows:

- intake
- orchestrator
- coder
- reviewer
- investigator

Each fixed role needs to select a Profile and, later, separate role-level system instruction, first launch prompt, and workflow binding. This is Ticket orchestration configuration, not a generic Profile replacement.

## Goal

Add workspace-local Ticket configuration at `.yoi/ticket.config.toml` and a typed parser/resolver for fixed Ticket role profile mappings.

The MVP should establish the configuration file, fixed role schema, backend root configuration, validation, and role-to-profile selector resolution. It should not yet spawn Pods or add TUI actions.

## Requirements

- Add typed Ticket orchestration config support for `.yoi/ticket.config.toml`.
- Keep roles fixed, not arbitrary:
  - `intake`
  - `orchestrator`
  - `coder`
  - `reviewer`
  - `investigator`
- Support backend configuration:
  - local backend kind;
  - root path, defaulting to `work-items` relative to the workspace.
- Support per-role configuration:
  - `profile` selector string;
  - optional role system instruction prompt reference;
  - optional launch/initial prompt reference;
  - optional workflow slug/reference.
- Keep `profile` selector syntax aligned with existing SpawnPod/profile selectors where possible:
  - `inherit`
  - `default`
  - `builtin:<name>`
  - `user:<name>`
  - `project:<name>`
  - unqualified registry selector when accepted by existing profile resolution.
- Preserve the conceptual separation:
  - Profile = Pod runtime recipe.
  - role system instruction = role's durable behavior/boundary.
  - launch prompt = first committed task/user message for a specific Ticket/action.
  - workflow = procedural flow, later potentially stateful.
- Validate known fields and reject/diagnose unknown roles or malformed fields.
- Resolve relative backend roots against workspace root.
- Do not auto-create backend directories in this ticket.
- Update the existing Ticket built-in feature adapter to use the configured backend root when available, falling back to `work-items`.
- Expose a reusable resolver API for later Pod launch/TUI code:
  - role -> profile selector;
  - role -> optional system instruction ref;
  - role -> optional launch prompt ref;
  - role -> optional workflow slug;
  - backend root.

## Non-goals

- Arbitrary role registry.
- Pod spawning or role launcher implementation.
- TUI action implementation.
- Stateful workflow engine.
- Per-phase workflow prompt injection.
- Changing Profile authoring/resolution semantics.
- Replacing `profiles.toml`.
- Renaming `work-items/`.
- External tracker integration.
- Scheduler/lease/queue automation.

## Suggested schema

```toml
[backend]
kind = "local"
root = "work-items"

[roles.intake]
profile = "project:intake"
system_instruction = "$workspace/ticket/intake/system"
launch_prompt = "$workspace/ticket/intake/launch"
workflow = "ticket-intake-workflow"

[roles.orchestrator]
profile = "project:orchestrator"
system_instruction = "$workspace/ticket/orchestrator/system"
launch_prompt = "$workspace/ticket/orchestrator/launch"
workflow = "ticket-orchestrator-routing"

[roles.coder]
profile = "inherit"
system_instruction = "$workspace/ticket/coder/system"
launch_prompt = "$workspace/ticket/coder/launch"
workflow = "multi-agent-workflow"

[roles.reviewer]
profile = "project:reviewer"
system_instruction = "$workspace/ticket/reviewer/system"
launch_prompt = "$workspace/ticket/reviewer/launch"
workflow = "multi-agent-workflow"

[roles.investigator]
profile = "inherit"
system_instruction = "$workspace/ticket/investigator/system"
launch_prompt = "$workspace/ticket/investigator/launch"
workflow = "ticket-orchestrator-routing"
```

MVP may make all role fields optional except `profile` when a role section is present. Missing file and missing role sections should fall back to builtin defaults.

## Default behavior

When `.yoi/ticket.config.toml` is absent:

- backend kind: local
- backend root: `<workspace>/work-items`
- all role profiles: `inherit`
- workflow defaults:
  - intake: `ticket-intake-workflow`
  - orchestrator: `ticket-orchestrator-routing`
  - coder: `multi-agent-workflow`
  - reviewer: `multi-agent-workflow`
  - investigator: `ticket-orchestrator-routing`
- system instruction / launch prompt refs: none

## Acceptance criteria

- `.yoi/ticket.config.toml` can be parsed from a workspace root.
- Missing config falls back to documented defaults.
- Fixed role sections parse correctly.
- Unknown roles are rejected or reported as configuration errors.
- Relative backend root resolves against workspace root.
- Backend root from config is used by the Ticket built-in feature adapter.
- Role profile selector strings are retained/parsed in a form later usable by role launching code.
- Optional `system_instruction`, `launch_prompt`, and `workflow` refs are parsed and exposed without trying to run a workflow engine.
- Tests cover missing config, full config, partial role config, unknown role, relative backend root, and adapter backend-root usage.
- `cargo test -p ticket` and focused `cargo test -p pod ticket --lib` pass.
- `cargo check --workspace --all-targets`, `cargo fmt --check`, `git diff --check`, and `./tickets.sh doctor` pass.

## Follow-up tickets

- `ticket-role-pod-launcher`: construct role-specific `SpawnPod` requests from Ticket context, role config, separated system instruction, launch prompt, workflow binding, and scope policy.
- `tui-ticket-role-actions`: expose fixed Ticket role actions in TUI using the launcher.
- Later workflow-state engine: persisted workflow phase/state, phase-specific allowed tools, and phase prompts committed to history before model use.
