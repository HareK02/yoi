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

<!-- event: state_changed author: workspace-panel at: 2026-06-25T16:39:39Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T16:44:20Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue による routing 許可を受けて Ticket / relations / orchestration plan / queue state を確認した。
- 本 Ticket は `00001KVZBCQH4` (`worker-runtime core crateと組み込みRuntime APIを作る`) に `depends_on` relation を持つ。
- `00001KVZBCQH4` は現在 `inprogress` で、implementation commits はあるが reviewer review 中。merge / Orchestrator validation / done ではない。
- REST command server は worker-runtime command/response/projection semantics に依存するため、core API 確定前に HTTP/server implementation side effect を開始しない。

Evidence checked:
- Ticket body: REST command server、request/response schema、operation timeouts、host/port config、diagnostics、Non-goals。
- Relations: outgoing `depends_on -> 00001KVZBCQH4`。incoming dependent `00001KVZ9JGK0`。
- Orchestration plan: blocker record `orch-plan-20260625-164410-1` を追加。
- Queue state: queued は本 Ticket、FS store、Backend Registry foundation、Backend embedded connection の4件。inprogress は `00001KVZBCQH4` 1件。

Next action:
- 本 Ticket は queued のまま待機。
- `00001KVZBCQH4` が reviewer approve / merge / validation / done になった後、再 routing する。

Escalate if:
- worker-runtime core の command/projection API が REST mapping に足りない。
- REST server concerns を core crate に混ぜないと acceptance を満たせないように見える。

---
