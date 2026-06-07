<!-- event: create author: LocalTicketBackend at: 2026-06-07T03:51:43Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-07T04:57:33Z -->

## Plan

## Preflight / implementation intent

Classification: implementation-ready first slice of Orchestrator automation.

Intent:
- Implement the queued Ticket routing entrypoint without starting coder/reviewer/worktree routing in this ticket.
- Panel Queue should notify the workspace Orchestrator with active routing semantics: read Ticket/workspace state and start if unblocked.
- Orchestrator workflow/prompt should define `queued -> inprogress` as the acceptance marker that must happen before implementation side effects.

Requirements:
- Update Panel Queue notice and Orchestrator notification text away from passive/implementation-not-started semantics.
- Notification must include Ticket id/slug/title where practical and instruct:
  - read the Ticket;
  - inspect current workspace state;
  - if unblocked, transition `queued -> inprogress` before worktree/SpawnPod side effects;
  - if blocked, record a concise reason and leave queued or explicitly defer.
- Update `ticket-orchestrator-routing` workflow guidance to reflect queued-trigger routing and the acceptance-order invariant.
- Do not implement worktree/coder/reviewer routing here; that belongs to `orchestrator-worktree-agent-routing`.
- Do not implement merge/close completion here; that belongs to `orchestrator-merge-completion`.
- Do not implement the orchestration plan tool here; `ticket-orchestration-plan-tool` owns durable ordering/dependency/conflict notes.

Current code map:
- `crates/tui/src/multi_pod.rs`
  - Queue action around `dispatch_ticket_action` currently says `No implementation was started.`
  - `notify_workspace_orchestrator` currently says `Do not start implementation directly from this notification; follow routing/preflight gates.`
  - Tests around Queue outcome and notify-only path are nearby.
- `.yoi/workflow/ticket-orchestrator-routing.md`
  - Currently says this workflow is not an unattended scheduler and requires human/upper Orchestrator permission before coder/reviewer spawn.
  - It should be updated so queued notifications are treated as human authorization to route; implementation side effects still require `queued -> inprogress` acceptance first.
- `crates/ticket/src/tool.rs`
  - `TicketWorkflowState` tool/backend already enforce allowed transitions; do not duplicate transition enforcement in panel text.

Validation:
- Update focused tests for Queue notice / notification wording.
- Run `cargo test -p tui multi_pod --lib`.
- Run prompt/resource relevant checks if available, otherwise `target/debug/yoi ticket doctor`, `cargo fmt --check`, and `git diff --check`.

---

<!-- event: intake_summary author: INSOMNIA at: 2026-06-07T04:57:43Z -->

## Intake summary

Implementation-ready first slice: update Panel Queue notification/notice and Orchestrator routing workflow so queued Tickets actively trigger Orchestrator routing, with `queued -> inprogress` acceptance required before worktree/SpawnPod side effects. Worktree/coder/reviewer and merge completion remain separate follow-up tickets.

---

<!-- event: state_changed author: INSOMNIA at: 2026-06-07T04:57:43Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket intake complete; workflow_state intake -> ready.


---
