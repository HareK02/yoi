<!-- event: create author: LocalTicketBackend at: 2026-06-21T07:11:34Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-21T07:14:05Z -->

## Intake summary

ユーザー指示により、この Ticket を Orchestrator が routing できる `ready` 状態にする。readiness は引き続き `requirements_sync_needed` であり、実装直行ではなく WebSocket capability の設計同期・仕様化として扱う。URL 権限を前提にし、`host_api.request` とは分離する binding decisions は Ticket body に記録済み。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-21T07:14:05Z from: planning to: ready reason: user_requested_ready field: state -->

## State changed

ユーザーから「readyにして」と明示されたため `planning` から `ready` へ遷移する。未決定点は残っているため、後続 Orchestrator routing では requirements/design sync として扱う。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-21T07:15:42Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-21T07:17:40Z -->

## Decision

Routing decision: `return_to_planning_requirements_sync`

Reason:
- Ticket frontmatter has `readiness: requirements_sync_needed`。
- Intake summary explicitly says this should be handled as WebSocket capability design/spec sync, not implementation-direct routing。
- Ticket body still lists concrete open questions that must be resolved before implementation or worktree side effects:
  - WebSocket connection ownership: Yoi host-owned vs Plugin-instance-owned host API。
  - Incoming message dispatch route: Ingress vs Service event/status stream vs other host routing。
  - Reconnect / backoff / heartbeat / shutdown / cancellation / restore scope for first slice。
  - Auth/headers/secrets model and grant/display split。
  - Whether this work item closes as design/spec only or includes a minimal implementation slice。
- Related Ticket `00001KVMG8FTW` is now routed for implementation of the `host_api.request` URL permission model. WebSocket design should reuse or intentionally diverge from that resulting URL permission schema after it stabilizes。

Decision:
- Do not create implementation worktree or spawn Coder/Reviewer for this Ticket now。
- Return to `planning` with concrete missing requirements/design decisions。
- Record relation to `00001KVMG8FTW` so WebSocket design can be resumed after request permission model lands。

Required next planning output before queueing again:
- Decide whether this is design/spec only or includes a minimal implementation slice。
- Decide connection ownership and lifecycle boundary。
- Decide incoming-message durable/visible dispatch path and no-hidden-context behavior。
- Decide WebSocket URL permission/grant schema relative to `host_api.request` request-target schema。
- Decide auth/secrets display/grant model and non-goals for reconnect/backoff/heartbeat in first slice。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-21T07:17:52Z from: queued to: planning reason: requirements_sync_needed_connection_lifecycle_and_scope_decisions field: state -->

## State changed

Ticket remains `requirements_sync_needed` and has unresolved design questions around connection ownership, incoming-message dispatch path, lifecycle bounds, auth/secrets handling, and design-vs-implementation scope. Returning to planning rather than starting implementation side effects.

---

<!-- event: intake_summary author: hare at: 2026-06-21T11:01:20Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-21T11:01:20Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---
