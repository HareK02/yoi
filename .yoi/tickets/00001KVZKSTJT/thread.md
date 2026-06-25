<!-- event: create author: "yoi ticket" at: 2026-06-25T14:44:02Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-25T16:34:16Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:34:16Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:42:14Z from: ready to: planning reason: cli_state field: state -->

## State changed

State changed to `planning`.


---

<!-- event: decision author: hare at: 2026-06-25T16:42:14Z -->

## Decision

Returned to planning because the current ticket is not concrete enough.

The purpose is specifically observation: Backend subscribes to a Runtime-owned WebSocket stream to receive Worker output and related runtime/worker events. It is not a command channel, not browser-facing, and not the path for sending user input.

Before this can be ready, define the event model and protocol boundary concretely:
- which Worker output events are streamed (text delta/final, reasoning visibility policy, tool call lifecycle, status, run started/completed/errored, usage, diagnostics);
- whether the stream is runtime-wide, worker-scoped, or both;
- event envelope shape, event id/cursor semantics, ordering, backlog, reconnect behavior, and unknown/expired cursor handling;
- relationship between streamed output and transcript projection/event log persistence;
- Backend client/proxy expectations and how Browser receives the projection without connecting directly to Runtime;
- what is deliberately excluded from the stream, such as raw provider trace or raw full session log.


---
