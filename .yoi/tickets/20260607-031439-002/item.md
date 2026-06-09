---
title: "Strictly validate Ticket role launch config"
state: "closed"
created_at: "2026-06-07T03:14:39Z"
updated_at: "2026-06-07T03:42:39Z"
---

## Background

Fallback audit found that Ticket backend config and Ticket role launch config are currently conflated. A backend-only `.yoi/ticket.config.toml` can be parsed and treated as usable by the panel, while missing role profiles are filled by role defaults that currently use `inherit`. Top-level Ticket role launches then fail late because `inherit` is not supported there.

The desired policy is fail-closed runtime validation: role launch must require explicit concrete role profile configuration. Init/scaffold may write builtin selectors explicitly, but runtime must not silently invent them.

## Goal

Add strict validation for Ticket role launch configuration so Panel Intake / Orchestrator launch availability reflects whether role config is actually executable.

## Requirements

- Separate Ticket backend availability from Ticket role launch availability.
  - Backend-only config may be usable for listing/showing Tickets.
  - Role launch actions require fixed role config validation.
- Add a role-launch validation path that rejects:
  - missing role table for the target fixed role;
  - missing role `profile`;
  - `profile = "inherit"` for top-level Ticket role launch;
  - role profile selectors that cannot be resolved for the launch path, where resolvability can be checked at this layer.
- Panel Intake and workspace Orchestrator lifecycle must use the strict role-launch validation before presenting/attempting launch.
- Diagnostics must be bounded and actionable, e.g. tell the user to run init/scaffold or set `[roles.<role>].profile` explicitly.
- Keep `TicketRoleLaunchPlan::spawn_config` rejection of `inherit` as a final defensive check, but normal Panel/launcher paths should fail earlier with a clearer diagnostic.
- Preserve explicit concrete builtin selectors when they are present in config; the target is not to ban builtin, only implicit fallback.
- Do not silently convert missing roles to `builtin:default`, `default`, or `inherit` at runtime.

## Non-goals

- Implementing init/scaffold generation; that belongs to `ticket-init-role-profile-scaffold`.
- Removing low-risk backend defaults such as the local Ticket backend root.
- Redesigning model/provider fallback policy.
- Allowing top-level Ticket role launch to resolve `inherit`.

## Acceptance criteria

- Backend-only `.yoi/ticket.config.toml` is not considered sufficient for Panel Intake / Orchestrator role launch.
- Missing or `inherit` role profile produces an early actionable diagnostic before spawn.
- Explicit concrete role profile config allows launch planning to proceed.
- Tests cover backend-only config, partial role config, explicit `inherit`, and full concrete role config.
- Existing Ticket backend listing/show behavior remains available when only backend config is present, unless a separate design intentionally changes that.
