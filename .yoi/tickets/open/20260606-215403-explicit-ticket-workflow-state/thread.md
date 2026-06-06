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
