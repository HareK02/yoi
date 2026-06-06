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
