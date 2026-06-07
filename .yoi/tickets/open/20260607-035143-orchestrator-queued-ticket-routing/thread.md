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

<!-- event: implementation_report author: hare at: 2026-06-07T04:58:26Z -->

## Implementation report

## Delegation

Implementation delegated to child Pod `orchestrator-routing-coder-20260607` in worktree `.worktree/orchestrator-queued-ticket-routing` on branch `work/orchestrator-queued-ticket-routing`.

Scope:
- Update Panel Queue notice and Orchestrator notification wording to actively trigger routing.
- Preserve the contract that implementation side effects occur only after `queued -> inprogress` acceptance.
- Update focused TUI tests.
- Do not implement worktree/coder/reviewer routing, merge completion, or the orchestration plan tool in this ticket.

Workflow note:
- Child worktree excludes `.yoi`, so workflow file updates under `.yoi/workflow` must be reported back for parent-side application rather than edited in the child worktree.

The child should commit implementation work in the child worktree and report diff/tests. Merge, review, Ticket closure, workflow file updates, and cleanup remain with the parent/human workflow.

---

<!-- event: implementation_report author: hare at: 2026-06-07T05:07:50Z -->

## Implementation report

## Implementation report

Coder Pod `orchestrator-routing-coder-20260607` completed the child-worktree code slice in `.worktree/orchestrator-queued-ticket-routing`.

Commit:
- `ccf43f8 tui: update queued ticket routing notification`

Reported summary:
- Updated Panel Queue notice so it says Orchestrator routing is authorized and implementation side effects still require `queued -> inprogress` acceptance.
- Updated Orchestrator notification text to include Ticket slug/id/title and instruct Orchestrator to read the Ticket, inspect workspace state, transition `queued -> inprogress` before worktree/SpawnPod side effects when unblocked, and record a concise blocked reason when blocked.
- Added focused tests for the routing notification contract.
- Did not implement worktree/coder/reviewer routing, merge completion, or plan-tool behavior.

Reported validation:
- `cargo test -p tui multi_pod --lib`
- `cargo fmt --check`
- `git diff --check`
- `nix build .#yoi`

Parent-side workflow update:
- Applied the reported `.yoi/workflow/ticket-orchestrator-routing.md` update in the main workspace because child worktrees exclude `.yoi`.
- The workflow now treats Panel Queue / queued notification as routing authorization, requires `queued -> inprogress` before implementation side effects, and documents blocked queued handling.

---

<!-- event: implementation_report author: hare at: 2026-06-07T05:08:38Z -->

## Implementation report

## Review delegation

External review delegated to `orchestrator-routing-reviewer-20260607`.

Review scope:
- Child implementation commit `ccf43f8 tui: update queued ticket routing notification` in `.worktree/orchestrator-queued-ticket-routing`.
- Parent-side workflow update in `.yoi/workflow/ticket-orchestrator-routing.md` committed on `develop` in `df6d7ee`.

Review focus:
- Queue notice/notification should actively authorize Orchestrator routing without implying passive no-op behavior.
- Notification must not instruct blind implementation Pod spawning.
- Workflow must require `queued -> inprogress` acceptance before worktree/SpawnPod side effects.
- Worktree/coder/reviewer routing, merge completion, and plan-tool behavior must remain out of scope.
- Focused tests should cover the changed wording/contract.

---

<!-- event: review author: orchestrator-routing-reviewer-20260607 at: 2026-06-07T05:12:59Z status: approve -->

## Review: approve

Approved.

Evidence:
- Panel Queue no longer presents passive/no-op semantics. It records `ready -> queued`, notifies the workspace Orchestrator, and reports that Orchestrator routing is authorized while implementation side effects still require `queued -> inprogress` acceptance.
- Orchestrator notification includes Ticket slug/id/title and instructs the Orchestrator to read the Ticket, inspect workspace state, transition `queued -> inprogress` before worktree/SpawnPod side effects when unblocked, and record a concise blocked/defer reason when blocked.
- Wording does not encourage blind implementation Pod spawning from notification alone.
- Parent-side `.yoi/workflow/ticket-orchestrator-routing.md` update matches the queued acceptance contract.
- No worktree/coder/reviewer/merge implementation leaked into this ticket; child diff is limited to `crates/tui/src/multi_pod.rs` wording/tests.
- Focused tests cover Queue notice and Orchestrator notification contract.

Reviewer validation:
- `cargo test -p tui multi_pod --lib`
- `cargo fmt --check`
- `git diff --check develop...HEAD`
- `nix build .#yoi`
- `git merge-tree --write-tree develop HEAD`

Merge readiness: approved; merge-tree against current `develop` succeeded. The workflow update is already present on `develop`.

---
