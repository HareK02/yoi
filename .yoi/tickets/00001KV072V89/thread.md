<!-- event: create author: ticket-intake at: 2026-06-13T10:05:19Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-13T10:05:28Z -->

## Intake summary

`yoi panel` の View item / row をマウスクリックでアプリ内選択できるようにする Ticket として materialize した。参考 UX は `./ghq.local/github.com/anomalyco/opencode` の OpenCode TUI selection/dialog/autocomplete 系。ratatui では DOM-like handler ではなく、描画時の item rect registry と `MouseEvent { column, row }` hit-test が有力。MVP は click/down による selection のみで、Queue/Open/Close 等の workflow action 即時実行、汎用 drag/text selection、single-Pod history block focus は範囲外。既存 composer 入力優先、selected-Pod direct-send 非復活、least-intrusive mouse capture 方針を invariant とする。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-13T10:05:28Z from: planning to: ready reason: planning_ready field: state -->

## State changed

要件、受け入れ条件、binding invariants、OpenCode 参考、実装裁量、escalation conditions、validation が整理され、Orchestrator が routing 可能な状態になったため ready にする。

---
