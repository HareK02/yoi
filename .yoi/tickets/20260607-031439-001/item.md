---
title: "Scaffold explicit Ticket role profiles in init config"
state: "closed"
created_at: "2026-06-07T03:14:39Z"
updated_at: "2026-06-07T04:05:15Z"
---

## Background

Fallback audit found that `.yoi/ticket.config.toml` can currently contain only backend settings. In that case Ticket config parsing succeeds, but fixed Ticket role profiles fall back to `inherit`, which top-level Panel/CLI Ticket role launch rejects. This causes Panel Intake / Orchestrator launch failures such as:

```text
Ticket role profile 'inherit' cannot be used for top-level launch execution; configure a concrete role profile selector
```

The desired policy is to distinguish explicit builtin preset selection from implicit runtime fallback. It is acceptable for init/scaffold to generate a working config that explicitly names builtin presets. It is not acceptable for runtime to silently fill missing role config with builtin/default/inherit.

## Goal

Make project init/scaffold generate a complete, explicit Ticket role configuration suitable for Panel Intake and Orchestrator launch.

## Requirements

- Locate or introduce the init/scaffold path that creates `.yoi/ticket.config.toml` for a workspace.
- Generate explicit backend config:
  - `[backend] provider = "builtin:yoi_local"`
  - `root = ".yoi/tickets"`
- Generate explicit fixed role tables for at least:
  - `[roles.intake]`
  - `[roles.orchestrator]`
  - `[roles.coder]`
  - `[roles.reviewer]`
  - `[roles.investigator]`
- Each role table must include a concrete `profile`, not `inherit`.
  - Minimal initial implementation may explicitly use `builtin:default` if role-specific builtin profiles are not ready yet.
  - If role-specific builtin profiles are introduced, use explicit selectors such as `builtin:ticket-intake`, `builtin:ticket-orchestrator`, etc.
- Each role table should include explicit `workflow` for readability and reproducibility:
  - intake: `ticket-intake-workflow`
  - orchestrator: `ticket-orchestrator-routing`
  - coder/reviewer: `multi-agent-workflow` or the agreed workflow for those roles
  - investigator: the agreed read/investigation workflow
- Generated config must pass Ticket role launch validation for Panel Intake and workspace Orchestrator.
- Do not implement runtime fallback from missing role profiles to builtin presets in this ticket.

## Non-goals

- Making builtin role profiles an implicit runtime fallback.
- Allowing `inherit` for top-level Ticket role launch.
- Redesigning provider/model fallback policy.
- Implementing full Companion lifecycle.

## Acceptance criteria

- A newly initialized workspace has explicit concrete Ticket role profiles in `.yoi/ticket.config.toml`.
- Panel Intake launch planning does not fail because generated role profile is `inherit` or missing.
- Workspace Orchestrator launch planning does not fail because generated role profile is `inherit` or missing.
- Tests cover generated config shape and successful role-launch validation against the scaffolded config.
