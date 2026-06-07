<!-- event: create author: "yoi ticket" at: 2026-06-07T00:16:51Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-07T00:18:58Z -->

## Decision

Decision from user discussion:

The panel should not provide direct messaging to arbitrary selected Pods. The existing `Companion` composer target is currently a misleading label for selected-Pod direct send and should be replaced by a real workspace Companion Pod.

Target model:
- default panel composer talks to a workspace-named Companion Pod;
- Companion is a foreground management chat for the human;
- Ticket Intake remains a separate target for new requests;
- selected Pod direct send is removed;
- Pod attach/open remains available for inspection;
- Companion should be status-aware but not directly write/mutate project state;
- Companion prompt/tool policy should focus on situational awareness and human support, with direct writes/Ticket mutations/implementation spawning prohibited by default.

Split child tickets were created for direct-send removal, Companion lifecycle, and Companion prompt/tool policy.


---

<!-- event: decision author: hare at: 2026-06-07T01:21:43Z -->

## Decision

## Local role/session overlay split

The Companion/panel redesign should not store Ticket↔local Pod assignment in git-tracked Ticket metadata. Local Pod names, runtime/session identity, socket/restorable state, and current claims are per-machine runtime concerns.

A new child ticket `workspace-panel-local-role-session-registry` covers the local overlay model:

- Ticket project records keep workflow state and auditable summaries only.
- A Ticket may have at most one active local Pod claim at a time.
- A Pod/session may relate to zero or more Tickets.
- Intake is not 1:1 with Ticket: pre-Ticket Intake may have no Ticket yet, and one Intake session may materialize/split into multiple Tickets.
- Existing `ticket-*` Pod visibility in the panel is acceptable as an interim access path until the registry UI/model exists.
- The panel should not poll and automatically start Intake for newly-created `workflow_state = intake` Tickets; starting/claiming remains an explicit action.

---

<!-- event: decision author: hare at: 2026-06-07T02:02:21Z -->

## Decision

## Orchestrator automation follow-up

Created `workspace-panel-orchestrator-queue-automation` to cover the missing end-to-end route from Panel Intake/Queue into Orchestrator-managed work.

This follow-up fixes the intended `queued` semantics:
- `queued` means the human authorized Orchestrator routing;
- the Orchestrator should inspect Ticket/workspace state and start if unblocked;
- conflicts, dependency blockers, preflight gaps, and capacity constraints are Orchestrator decisions, not reasons for the panel to stay passive forever.

This remains separate from Companion lifecycle and local role/session registry work, but may consume the registry once available to avoid duplicate ownership.

---
