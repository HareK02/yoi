<!-- event: create author: "yoi ticket" at: 2026-07-02T13:59:17Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-02T14:00:20Z -->

## Plan

Workspace Browser Settings shell の先行 Ticket として詳細化した。

方針:
- Settings は admin/user/permission 管理ではなく、現在の local Workspace Backend process と workspace-local config を扱う surface とする。
- Runtime Connections / Backend Config / Workspace Identity の section 受け皿を先に作る。
- この Ticket では mutation や Runtime connection 永続化は実装しない。
- 後続 Runtime connection 管理 Ticket `00001KWHHRTM9` はこの Settings shell を前提にする。


---

<!-- event: decision author: hare at: 2026-07-02T14:09:54Z -->

## Decision

Settings shell Ticket の位置づけを修正した。

Decision:
- `Workspace-local Settings` という独自呼称はやめ、普通に Settings/Admin surface として扱う。
- 管理画面ではない、という意味ではない。
- 現時点で user / permission / multi-user authorization が無いため、それらがあるかのような fake UI / 文言を作らない、という制約だけを明記する。
- 後続 Runtime connection management Ticket の依存先表記も Settings/Admin shell に修正した。


---

<!-- event: intake_summary author: hare at: 2026-07-02T14:18:50Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-02T14:18:50Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---
