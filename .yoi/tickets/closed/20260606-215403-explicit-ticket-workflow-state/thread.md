<!-- event: create author: yoi ticket at: 2026-06-06T21:54:03Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-06T21:54:48Z -->

## Plan

Created from design discussion about removing inferred panel Ticket state.

Decision recorded:
- stop treating panel heuristics from labels/title/thread events as authoritative workflow state;
- add an explicit durable Ticket workflow state: `intake -> ready -> queued -> inprogress -> done`;
- `intake -> ready` is completed through Intake Pod conversation/materialization;
- `ready -> queued` is the normal human panel action, replacing the current `Go` action with `Queue`;
- review/rework stay inside `inprogress`;
- blocked/user-attention is an overlay such as `attention_required`, not a workflow state;
- transient activity should be derived from live Pod/session/thread state and not persisted in Ticket frontmatter.


---

<!-- event: decision author: hare at: 2026-06-06T22:04:15Z -->

## Decision

Updated per user direction:

- Ticket row should not expose priority/action/status/phase as separate permanent columns; they are redundant once explicit workflow state exists.
- Ticket rows should be essentially `state + slug/id + title`.
- Pod rows should likewise be essentially `pod-state + pod-name`.
- Operations such as Queue/Defer/Open/Send should move to selected-row actionbar/key hints instead of row columns.
- Composer/status bar should be kept concise; verbose target/help/diagnostic text should move to actionbar or detail/diagnostic areas.


---

<!-- event: decision author: hare at: 2026-06-06T22:14:29Z -->

## Decision

Updated based on thread/event-log discussion and split out companion work.

Decision:
- Do not delete Ticket threads.
- Do not use threads as freeform AI conversation dumps.
- Use `item.md` frontmatter for current workflow state, `item.md` body for the current Ticket snapshot, `thread.md` as a concise typed append-only event log, and Pod/session logs for full conversations.
- Workflow state transitions should eventually update frontmatter and append a `state_changed` event as one logical backend operation.
- Intake should write a bounded `intake_summary` when materializing/marking a Ticket ready, not copy the full Intake conversation.

Created companion ticket `typed-ticket-thread-event-log` for the typed thread event model/API so `explicit-ticket-workflow-state` can stay focused on current-state fields and panel semantics.


---

<!-- event: plan author: hare at: 2026-06-06T22:49:29Z -->

## Plan

Preflight result: `implementation-ready` after `typed-ticket-thread-event-log`.

This ticket should add explicit durable Ticket workflow fields and update the panel to stop using inferred state/action/status columns. The core workflow is `intake -> ready -> queued -> inprogress -> done`; `ready -> queued` is the normal human panel action and should be shown as `Queue`, not `Go`.

Implementation should use the newly-added typed thread event APIs for state transitions where practical, simplify panel rows to state + identity/title, and keep transient Pod activity out of Ticket frontmatter.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-07T00:08:04Z status: approve -->

## Review: approve

External reviewer approved current HEAD after two requested-changes cycles.

Review summary:
- Explicit workflow fields and panel display are implemented.
- Ticket rows are state + id/title oriented and no longer use inferred priority/action/status/phase as main-list authority.
- Queue re-checks current state and transitions ready -> queued through the dedicated backend path.
- Typed tools expose bounded Intake and Orchestrator workflow transitions.
- Workflow transition graph is centrally enforced:
  - IntakeReady only permits intake -> ready.
  - Queue path owns ready -> queued.
  - Role-side TicketWorkflowState permits queued -> inprogress and inprogress -> done.
  - Backward/skip/bypass transitions are rejected.
- Handoff prompt uses workflow_state / Queue vocabulary instead of old readiness/Go wording.
- No transient activity is persisted in frontmatter.


---

<!-- event: state_changed author: hare at: 2026-06-07T00:08:04Z from: intake to: done reason: closed field: workflow_state -->

## State changed

Ticket closed; workflow_state set to done.


---

<!-- event: close author: hare at: 2026-06-07T00:08:04Z status: closed -->

## Closed

Implemented explicit Ticket workflow state.

Final frontmatter fields:
- `workflow_state: intake | ready | queued | inprogress | done`
- `attention_required: null | "..."`
- `queued_by: null | "..."`
- `queued_at: null | "..."`

State/default behavior:
- Closed Tickets default/derive to `done` where appropriate.
- Existing non-closed Tickets without explicit workflow state use conservative defaults without treating labels/title/thread heuristics as workflow-state authority.
- Transient activity such as reviewing/reworking/validating is not persisted in frontmatter.

Workflow transition APIs/tools:
- `mark_intake_ready` / `TicketIntakeReady` performs `intake -> ready`, appending typed `intake_summary` and `state_changed` events.
- `queue_ready` remains the dedicated panel Queue path for `ready -> queued`, sets queued metadata, and appends typed `state_changed`.
- `set_workflow_state` / `TicketWorkflowState` is bounded to role-side transitions `queued -> inprogress` and `inprogress -> done`.
- Generic `set_state_field(..., "workflow_state", ...)` is rejected to prevent bypass.
- Backward/skip transitions such as `ready -> inprogress`, `queued -> done`, and `done -> intake` are rejected.

Panel changes:
- Panel rows display explicit workflow state directly.
- Ticket rows are simplified to state + slug/id + title.
- Pod rows are simplified to pod-state + pod-name.
- Row operations move to selected-row actionbar/key hints instead of permanent action/status/phase columns.
- Queue replaces the previous Go/ApproveIntake wording and is only valid for current `workflow_state == ready`.
- Queue notifies Orchestrator when reachable; notification failure does not roll back a successful Ticket transition.
- No-Ticket Pod-centric panel behavior is preserved.

Role prompt/tool behavior:
- Intake/Orchestrator role text now uses `workflow_state` / `Queue` vocabulary.
- Intake is instructed to set `workflow_state = ready` through typed Ticket tools after materializing a Ticket.
- Orchestrator treats `queued` as schedulable and moves to `inprogress` when starting.

Validation after merge:
- `cargo test -p ticket workflow --lib`
- `cargo test -p ticket`
- `cargo test -p tui workspace_panel --lib`
- `cargo test -p tui multi_pod --lib`
- `cargo test -p yoi panel`
- `cargo test -p yoi ticket`
- `cargo test -p pod ticket --lib`
- `cargo test -p client ticket_role`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

External review approved after transition-graph enforcement was added.


---
