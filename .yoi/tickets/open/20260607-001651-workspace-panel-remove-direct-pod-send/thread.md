<!-- event: create author: "yoi ticket" at: 2026-06-07T00:16:51Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-07T01:32:12Z -->

## Plan

## Preflight / implementation intent

Classification: implementation-ready.

Intent:
- Remove the workspace panel path that treats `Companion` composer input as a direct `Method::Run` send to the selected Pod.
- Keep Pod open/attach, Ticket Intake launch, Orchestrator lifecycle, Queue/Ticket actions, and Pod discovery intact.
- Until the real Companion Pod lifecycle exists, Companion composer submit should produce a bounded diagnostic instead of falling back to selected-Pod direct send.

Requirements:
- `Enter` on non-empty Companion composer input must not send to `selected_pod.socket_path`.
- UI/status/key hints must stop advertising selected-Pod direct send.
- Pod rows should advertise/open attach behavior, not direct-send behavior.
- Ticket Intake target must still build and launch Intake requests.
- Existing no-Ticket panel remains useful for discovering and opening Pods.

Invariants / boundaries:
- Do not implement real Companion Pod lifecycle here; that belongs to `workspace-panel-companion-pod-lifecycle`.
- Do not change Companion tool/profile policy here; that belongs to `companion-status-context-tool-policy`.
- Do not remove Pod attach/open.
- Do not reintroduce single-Pod `:ticket` or old `--multi` semantics.
- Do not change Ticket workflow-state semantics.

Current code map:
- `crates/tui/src/multi_pod.rs`: `MultiPodApp::prepare_send`, `send_run_and_confirm`, Enter handling, status/help text, and tests currently embody selected-Pod direct send.
- `crates/tui/src/workspace_panel.rs`: `NextUserAction::SendToPod`, `pod_row`, and key hints currently advertise direct-send behavior for Pod rows.
- `crates/tui/src/multi_pod.rs`: `prepare_intake_launch` / `launch_intake_with_handoff` must remain intact for Ticket Intake.

Critical risks:
- Accidentally removing/weakening open/attach while deleting send behavior.
- Leaving stale status-bar/help text that still claims Enter sends to selected Pods.
- Conflating this cleanup with real Companion lifecycle and creating a fake/partial Companion implementation.
- Breaking Ticket Intake Enter behavior while removing Companion direct send.

Validation:
- Update focused `tui` unit tests that currently assert direct-send eligibility and non-empty Enter direct-send behavior.
- Run focused tests around `multi_pod` and `workspace_panel`, plus formatting/checks as appropriate for the diff.

---

<!-- event: intake_summary author: INSOMNIA at: 2026-06-07T01:32:18Z -->

## Intake summary

Implementation intent is clear: remove selected-Pod direct send from the workspace panel, keep Pod open/attach and Ticket Intake, and show a bounded diagnostic for Companion composer submit until the real Companion Pod lifecycle is implemented.

---

<!-- event: state_changed author: INSOMNIA at: 2026-06-07T01:32:18Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket intake complete; workflow_state intake -> ready.


---

<!-- event: implementation_report author: hare at: 2026-06-07T01:33:27Z -->

## Implementation report

## Delegation

Implementation delegated to child Pod `remove-direct-pod-send-coder-20260607` in worktree `.worktree/workspace-panel-remove-direct-pod-send` on branch `work/workspace-panel-remove-direct-pod-send`.

Scope:
- Remove selected-Pod direct send from `yoi panel` Companion composer behavior.
- Keep Pod open/attach and Ticket Intake launch working.
- Show a bounded diagnostic for Companion composer submit until real Companion lifecycle exists.
- Update focused `multi_pod` / `workspace_panel` tests and run focused validation.

The child should commit implementation work in the child worktree and report diff/tests. Merge, review, Ticket closure, and cleanup remain with the parent/human workflow.

---
