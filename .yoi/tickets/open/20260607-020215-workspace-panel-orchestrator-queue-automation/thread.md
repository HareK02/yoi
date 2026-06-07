<!-- event: create author: LocalTicketBackend at: 2026-06-07T02:02:15Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-07T02:35:20Z -->

## Decision

## In-progress acceptance contract

`workflow_state = inprogress` should be the durable Orchestrator acceptance marker, not merely proof that a coder Pod is already running.

Contract:
- `ready`: materialized Ticket waiting for the human Queue gate.
- `queued`: human has authorized Orchestrator routing; Orchestrator should inspect Ticket/workspace state and start if unblocked.
- `inprogress`: Orchestrator has accepted the queued Ticket and owns coordination/progress/block/failure reporting until completion or explicit defer/block.

Ordering invariant:
- Orchestrator must not create implementation worktrees or spawn implementation/review Pods for a queued Ticket unless the Ticket has already been accepted as `inprogress`, or the same typed operation atomically accepts it.
- If `queued -> inprogress` fails, do not spawn implementation Pods.
- If side effects fail after `inprogress` acceptance, record the failure/block under the in-progress Ticket rather than silently returning to queued or starting a duplicate path.

Short-term implementation may enforce this through Orchestrator workflow/prompt text plus the existing typed workflow-state tool. Longer-term, prefer a typed Orchestrator operation such as `AcceptQueuedTicket` / `StartTicketWork` that validates `queued`, records the acceptance transition, establishes any local claim/lease, and only then allows worktree/Pod launch.

---

<!-- event: decision author: hare at: 2026-06-07T03:35:44Z -->

## Decision

## Split: Ticket lifecycle domain feature

Created `ticket-lifecycle-pod-feature` to pull the domain tool/feature part out of Orchestrator automation.

Decision:
- Ticket lifecycle should be implemented as a Ticket-domain `pod::feature`, not an Orchestrator feature.
- Features represent domain capabilities; roles/profiles decide which subset/authority they receive.
- Orchestrator automation should consume the Ticket lifecycle feature for queued Ticket inspection and lifecycle transitions.
- Companion/Intake/coder/reviewer may receive different Ticket feature subsets according to their policies.

This keeps `workspace-panel-orchestrator-queue-automation` focused on routing behavior, queued notification handling, acceptance sequencing, and worktree/Pod orchestration rather than basic Ticket tool registration.

---
