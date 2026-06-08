<!-- event: create author: LocalTicketBackend at: 2026-06-08T10:31:33Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-08T10:31:40Z -->

## Intake summary

チャットビューの TUI で Markdown pipe table が読み取り可能に描画できない不具合を bug Ticket として整理した。対象 surface は通常の chat / conversation view に限定し、session history / Pod metadata / worker history / prompt context は変更しないことを invariant とした。通常の pipe table 表示、既存 Markdown/plain text の非退行、狭い terminal での安全な表示、関連テストまたは手動確認を受け入れ条件にしている。

---

<!-- event: state_changed author: intake at: 2026-06-08T10:31:40Z from: planning to: ready reason: planning_ready field: workflow_state -->

## State changed

要件・受け入れ条件・invariant・validation が揃っており、Orchestrator が implementation routing できる状態になった。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T13:13:23Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
