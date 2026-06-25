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

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T20:21:27Z from: ready to: planning reason: websocket_transport_decision_missing field: state -->

## State changed

ユーザー指示により planning に戻す。

Missing decision / information:
- WebSocket / event-stream transport は `00001KVZKSTJT` 自体で決定すべき設計点であり、未決定のまま ready/queue 対象として扱うのは不適切。
- 少なくとも、Backend-owned WebSocket client 方式を v0 で採用するか、SSE / polling / Backend proxy projection との責務分離をどう置くか、cursor/backlog/error semantics をどこまで固定するかを planning で再確認する必要がある。

Context checked:
- Ticket body: `worker-runtimeにWebSocket event stream serverを追加する` は `ws-server` feature、WebSocket observation endpoint、cursor resume、unknown/expired cursor diagnostics を実装対象としている。
- Relations: `00001KVZBCQH4` と `00001KVZKSTE2` に depends_on、`00001KVZSGT14` が本 Ticket に depends_on。
- Current state: 本 Ticket は queued ではなく `ready` だったが、WS を扱う予定の Ticket として routing/queue 前に設計判断へ戻す。

Why implementation latitude is insufficient:
- Transport choice / ownership boundary / Browser direct access exclusion / Backend proxy shape は local implementation tactic ではなく、後続 Backend/remote Runtime/Web Console の設計前提になる binding decision。

Next planning question/action:
- `worker-runtime` observation transport は v0 で WebSocket を採用するのか、それとも SSE/polling/Backend projection を優先するのか。
- WebSocket を採用する場合、Backend-owned client、cursor/backlog/unknown cursor、worker-scoped filtering、Browser-facing protocol non-goal の境界を明文化する。
- 後続 `00001KVZSGT14` など remote observation 依存 Ticket は、この判断後に readiness/relations を再確認する。

---
