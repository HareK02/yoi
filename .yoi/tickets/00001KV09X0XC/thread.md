<!-- event: create author: intake at: 2026-06-13T10:54:34Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-13T10:54:41Z -->

## Intake summary

ユーザー承認済みの新規 Ticket。Panel の `ready` Ticket に対し、Queue 以外にユーザー指示付きで `planning` へ戻し、同じ Ticket の Intake を restore / launch して再詳細化できる操作を追加する。実装開始ではなく requirements sync / refinement 導線であり、typed Ticket backend 経由の `ready -> planning` 記録、stale state 再確認、指示の thread 永続化、Intake 起動結果の bounded 表示、Queue / Orchestrator / worktree / coder/reviewer side effect 不発生が受け入れ条件。

---

<!-- event: state_changed author: intake at: 2026-06-13T10:54:41Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement completed. ユーザーが draft を承認し、意図・受け入れ条件・binding invariants・implementation latitude・escalation conditions・validation が Orchestrator routing 可能な粒度で揃っている。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-13T16:33:26Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
