---
id: '20260608-054546-shutdown-intake-pod-after-ready-idle'
slug: 'shutdown-intake-pod-after-ready-idle'
title: 'Shutdown Intake Pod after TicketIntakeReady returns to Idle'
status: 'closed'
kind: 'task'
priority: 'P2'
labels: ['ticket', 'intake', 'pod', 'lifecycle', 'panel']
workflow_state: 'done'
created_at: '2026-06-08T05:45:46Z'
updated_at: '2026-06-08T08:12:36Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T06:20:43Z'
---

## Background

Ticket Intake Pods launched from the workspace panel can complete their purpose by calling `TicketIntakeReady`, which appends an intake summary and transitions the Ticket from `intake` to `ready`. After that, the Intake Pod often remains live even though its Ticket-specific work is complete.

The local claim/session record should remain: it records which Intake Pod handled the Ticket and allows later inspection/restoration. The missing behavior is not claim release; it is automatically stopping the completed Intake Pod after its current turn settles.

Desired behavior:

```text
Ticket Intake Pod calls TicketIntakeReady successfully
  -> mark this Pod for shutdown-after-idle
  -> allow tool result, assistant response, history/session commits, and turn cleanup to complete
  -> when the Pod returns to Idle, cleanly Shutdown the Pod
  -> claim remains and appears as stopped/restorable
```

## Goal

Automatically shut down Ticket Intake role Pods after a successful `TicketIntakeReady` once the Pod returns to Idle.

## Requirements

- Detect successful `TicketIntakeReady` calls made by a Pod that is actually running as a Ticket Intake role/session.
- Schedule self-shutdown for the next Idle transition, not during the tool call itself.
  - The assistant should be able to receive the tool result and produce the final response.
  - History/session state should be committed normally.
  - Shutdown should happen only after the turn has fully settled and the Pod is Idle.
- Do not shut down on failed `TicketIntakeReady` calls.
- Do not shut down Companion, Orchestrator, Coder, or Reviewer Pods merely because they invoked `TicketIntakeReady` or an equivalent Ticket tool path.
- Do not release/delete the local Ticket claim as part of this behavior.
  - The claim/session record should remain for audit and later inspection.
  - Panel should show it as stopped/restorable if the Pod can be restored.
- Ensure the shutdown is clean and uses the same lifecycle path as ordinary Pod shutdown where practical.
- Surface bounded diagnostics if the post-idle shutdown cannot be scheduled or fails.
- Avoid killing the Pod before protocol/tool results have been delivered to the user/TUI.

## Design considerations

- The signal probably cannot live only in the `ticket` crate tool implementation, because shutdown is Pod/controller lifecycle behavior.
- Prefer a Pod-side adapter/hook or tool-result metadata path that can request `shutdown_after_idle` when:
  - tool name is `TicketIntakeReady`;
  - tool result indicates success;
  - current Pod role/session is Ticket Intake.
- The state should be transient runtime state, not model-visible instruction text.
- If the Pod receives another user turn before the shutdown runs, define whether shutdown still proceeds or is cancelled; prefer deterministic behavior and tests.

## Acceptance criteria

- A Ticket Intake Pod that successfully calls `TicketIntakeReady` shuts down after returning to Idle.
- The final assistant response/tool result from the successful turn is preserved before shutdown.
- Failed `TicketIntakeReady` does not schedule shutdown.
- Non-Intake Pods invoking Ticket tools do not self-shutdown.
- Local Ticket claim/session record remains after shutdown.
- Panel shows the completed Intake claim as stopped/restorable where metadata allows.
- Tests cover success, failure, role mismatch, and ordering around Idle transition.
- Focused tests, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
