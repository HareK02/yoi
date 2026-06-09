---
title: "Workspace panel local role session registry"
state: "closed"
created_at: "2026-06-07T01:21:31Z"
updated_at: "2026-06-07T02:34:48Z"
---

## Background

The panel now has two Ticket/Intake entry paths:

- A user instruction can start a pre-Ticket Intake session from the panel composer.
- An already-existing Ticket in `workflow_state = intake` can later need an Intake session.

For now, `ticket-*` Pods being visible in the panel is an acceptable interim access path. Longer term, the panel needs an explicit local model for role sessions and Ticket claims.

Ticket metadata and thread files are git-managed project records. Local Pod names, session/socket/runtime state, and per-machine assignment state must not be written into Ticket frontmatter or other git-tracked Ticket records.

Intake sessions are not necessarily 1:1 with Tickets. A pre-Ticket Intake session may have no Ticket yet, may materialize one Ticket, or may split the request into multiple Tickets. Conversely, an existing Ticket may be clarified by a later Intake session.

## Goal

Introduce a local, workspace-scoped role session registry and Ticket claim index for panel orchestration state, without storing local Pod assignment details in git-tracked Ticket metadata.

## Target model

- Ticket project records keep durable workflow state such as `workflow_state`, `attention_required`, and review/summary events.
- Local Pod/session assignment lives in a user-data workspace overlay, not under git-tracked `.yoi/tickets` files.
- A Ticket may have at most one active local Pod claim at a time.
- A Pod/session may relate to zero or more Tickets.
- Pre-Ticket Intake sessions are represented as local role sessions even before any Ticket exists.
- Existing-Ticket Intake uses the Ticket claim index to prevent a second Pod from attaching to an already-claimed Ticket.

## Requirements

- Define a workspace-scoped local storage location for panel role/session overlay data under the user data directory.
- Model local role sessions keyed by Pod/session identity, including at least role, pod name, origin, created/updated timestamps, and related Ticket refs.
- Model Ticket claims keyed by stable Ticket id, with at most one active local Pod claim per Ticket.
- Do not write local Pod names, socket paths, runtime status, or local claim state into git-tracked Ticket frontmatter/thread files.
- Allow one Intake Pod/session to relate to multiple Tickets.
- Allow a pre-Ticket Intake session to exist with no related Ticket.
- When starting Intake for an existing Ticket, claim before spawning/restoring so double-spawn races are avoided.
- If a Ticket already has a local claim, do not start a second Pod automatically.
  - If the claimed Pod is live, offer/open/attach that Pod.
  - If the claimed Pod is restorable, offer restore/open.
  - If the claim is stale, show an explicit reclaim action/diagnostic rather than silently starting a second Pod.
- Panel should join Ticket state, local overlay, and Pod metadata for display/actions.
- Do not introduce polling that automatically starts Intake for newly-created `workflow_state = intake` Tickets.
- Keep the current `ticket-*` Pod visibility as an acceptable interim access path until the registry UI is implemented.

## Non-goals

- Storing local Pod assignment in Ticket metadata.
- Assuming Intake Pod and Ticket are 1:1.
- Automatically starting Intake by polling Ticket files.
- Replacing the Orchestrator scheduling model.
- Full cross-machine coordination; this is a local runtime overlay.

## Acceptance criteria

- Local role session / Ticket claim data is stored outside git-tracked Ticket records.
- A Ticket cannot be claimed by two active local Pods through panel actions.
- A single Intake Pod can be associated with multiple Tickets in the local model.
- A pre-Ticket Intake session can be represented before any Ticket exists.
- Panel can distinguish at least: no claim, live/restorable claimed Pod, and stale claim.
- Existing tests or new tests cover the non-1:1 Intake/Ticket relation and one-active-claim-per-Ticket invariant.
