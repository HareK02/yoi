<!-- event: create author: "yoi ticket" at: 2026-06-25T14:44:03Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-25T16:27:28Z -->

## Decision

Decision update: split Backend Runtime work into three implementation tickets.

1. 00001KVZKSV6C Backend RuntimeRegistryの基盤をworker-runtime向けに整理する
   - Registry identity/projection/error boundary only.
   - No embedded Runtime handle implementation.
   - No remote Runtime client implementation.
2. 00001KVZSGT0Q Backend RuntimeRegistryにembedded worker-runtimeを接続する
   - Direct lib-call integration for backend-internal Runtime.
3. 00001KVZSGT14 Backend RuntimeRegistryにremote worker-runtime processを接続する
   - HTTP/event-stream client integration for remote Runtime processes.

Companion Web Console depends on embedded integration, not remote integration.


---

<!-- event: intake_summary author: hare at: 2026-06-25T16:30:31Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:30:31Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T16:31:28Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T16:32:17Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue による routing 許可を受けて Ticket / relations / orchestration plan / workspace state を確認した。
- 本 Ticket は `00001KVZBCQH4` (`worker-runtime core crateと組み込みRuntime APIを作る`) に `depends_on` relation を持つ。
- `00001KVZBCQH4` は現在 `inprogress`。Coder implementation report は届いているが、まだ review / merge / validation / done ではない。
- 本 Ticket の要件は worker-runtime core domain types を前提に Backend Registry foundation を整理する内容なので、依存 Ticket が確定する前に別 worktree / Coder Worker を開始すると API churn / conflict risk が高い。

Evidence checked:
- Ticket body: Backend RuntimeRegistry foundation、runtime_id / worker_id authority、existing local compatibility source、workspace-server hosts/server target、Non-goals。
- Relations: outgoing `depends_on -> 00001KVZBCQH4`; incoming dependents `00001KVZSGT0Q`, `00001KVZSGT14`。
- Orchestration plan: blocker record `orch-plan-20260625-163206-1` を追加。
- Workspace state: `00001KVZBCQH4` implementation worktree exists and coder has reported commit `9b2cae32`, but no reviewer approval / Orchestrator merge yet。
- Queue state: 本 Ticket と `00001KVZSGT0Q` が queued、`00001KVZBCQH4` が inprogress。

Next action:
- 本 Ticket は queued のまま待機。
- `00001KVZBCQH4` が reviewer approve / merge / validation / done になった後、再 routing して unblocked なら `queued -> inprogress` acceptance に進む。

Escalate if:
- `00001KVZBCQH4` の Runtime API shape が本 Ticket の前提を満たさない。
- Backend Registry foundation 側で worker-runtime core の追加変更が必要になる。

---
