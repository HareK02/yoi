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

<!-- event: decision author: hare at: 2026-06-25T20:05:49Z -->

## Decision

Runtime WebSocket event stream は、新しい Worker output protocol を作らず `crates/protocol` の `protocol::Event` を Backend-facing observation payload として流す方針にする。Runtime WS は `protocol::Event` の variant allowlist / subset を定義せず、worker-scoped envelope に event id / cursor / worker id を付けて Worker event bus の `protocol::Event` を forward する。

Browser / Web UI は Runtime WS に直接接続しない。Backend が Runtime WS client になり、Browser-facing stream は Backend-owned projection layer を通す。この projection layer を、後続の Web 権限制御で observation allow/deny、thinking/tool output/diagnostic redaction、operation-capable command API forwarding allow/deny を差し込む境界にする。STJT では full auth model は実装せず、この seam を型と責務として作る。


---

<!-- event: intake_summary author: hare at: 2026-06-25T20:10:54Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T20:10:54Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T20:24:40Z from: ready to: planning reason: cli_state field: state -->

## State changed

State changed to `planning`.


---

<!-- event: decision author: hare at: 2026-06-25T20:25:52Z -->

## Decision

STJT の主目的を Runtime WS server 単体から `Runtime -> Backend -> Client` の WebSocket observation proxy に広げる。Remote Runtime から Backend Runtime WS client が `protocol::Event` を受け取り、Backend-owned client-facing WS で `runtime_id + worker_id` keyed stream として流すところまでをこの Ticket の実装対象に含める。

Backend proxy は v0 では pass-through projection を基本にし、`protocol::Event` を別 output model へ変換しない。一方で Browser / future TUI は Runtime WS に直接接続せず、Backend-owned cursor/envelope/diagnostic を持つ stream だけを見る。Runtime endpoint / credential / socket path / session path は Client-facing authority に出さない。

権限系はこの Ticket で full auth / permission / redaction rule を実装しない。Backend proxy/projection layer に、後続で observe allow/deny、thinking/tool output/diagnostic redaction、action affordance、command API forwarding allow/deny を差し込める seam を作るに留める。


---
