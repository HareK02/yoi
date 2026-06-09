<!-- event: create author: LocalTicketBackend at: 2026-06-09T03:25:33Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-09T04:43:46Z -->

## Intake summary

既存 Ticket は、session JSONL を read-only に解析する再利用可能な `session-analytics` crate と薄い `yoi session analyze <path> --json` CLI を追加する concrete work item として十分に具体化済み。初期対象 metrics、raw content を既定で出さない privacy/secret 境界、non-goals、受け入れ条件、検証項目が明記されており、Orchestrator が実装 routing を判断できる。関連する `prompt-eval-metrics` は評価/改善 offer 寄りであり、本 Ticket は低レベル session/tool analytics 基盤として重複ではない。

---

<!-- event: state_changed author: intake at: 2026-06-09T04:43:46Z from: planning to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake で要件が実装 routing 可能な粒度まで整理済みであることを確認したため、planning から ready にします。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T06:56:20Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
