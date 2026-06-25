<!-- event: create author: "yoi ticket" at: 2026-06-25T19:32:38Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-25T19:56:23Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T19:56:23Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T20:34:42Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T20:36:43Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue 後に Ticket / relations / workspace state を確認した。
- 本 Ticket は Runtime API / WebSocket observation への TUI移行であり、Backend RuntimeRegistry foundation、embedded/remote Runtime routing、WebSocket observation proxy が前提。
- 現在 `00001KVZKSV6C` は inprogress、`00001KVZKSTJT` / `00001KVZSGT0Q` / `00001KVZSGT14` は queued/blocked。TUI migration を先に始めると transport/API の未確定部分を TUI 側で固定してしまうため開始しない。

Evidence checked:
- Ticket body: TUI connection model、input path、output/observation path、Runtime WebSocket / Backend proxy reliance、compatibility/debug path。
- Relations: outgoing dependencies include `00001KVZKSTJT`, `00001KVZKSV6C`, `00001KVZSGT0Q`, `00001KVZSGT14`。
- Orchestration plan: blocker record `orch-plan-20260625-203613-1` を追加。

Next action:
- 本 Ticket は queued のまま待機。
- Backend RuntimeRegistry / embedded+remote Runtime / WS proxy chain が done になった後に再 routing する。

---
