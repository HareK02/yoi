<!-- event: create author: ticket-intake at: 2026-06-18T14:09:59Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-18T14:10:12Z -->

## Intake summary

ユーザー承認済み。`yoi panel` の通常 Pod 表示を現在の runtime workspace に属する Pod だけに限定する concrete work item。workspace 外 Pod は通常一覧/action target から除外し、workspace 判定不能な legacy/corrupt metadata は通常表示せず bounded diagnostic のみ許容する。受け入れ条件・binding invariants・validation は Ticket body に記録済み。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-18T14:10:12Z from: planning to: ready reason: user_approved_intake_ready field: state -->

## State changed

Ticket intake が完了しました。実装起動は Orchestrator routing / queue flow に委ねます。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-18T14:47:10Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
