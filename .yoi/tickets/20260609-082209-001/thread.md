<!-- event: create author: LocalTicketBackend at: 2026-06-09T08:22:09Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-09T10:20:48Z -->

## Intake summary

既存 Ticket を再読し、duplicate/related work を確認した。これは既存の `pod::feature` registry、Task built-in feature、Ticket built-in feature、Ticket orchestration tools を前提に、Lua Profile / resolved Manifest から feature 単位で LLM tool surface を登録抑制する concrete implementation work item であり、新規 duplicate Ticket は不要。意図、対象 feature groups、core tools の非対象化、disabled feature は permission deny ではなく schema から消す方針、role default、受け入れ条件、検証が十分明確。blocking open question はなし。主な risk/reviewer focus は `profile/manifest`, `feature-boundary`, `authority-boundary`, `tool-schema`, `prompt-guidance`, `web-config-fail-closed`。`feature.ticket.access` / `feature.memory.access` の粒度は、既存 `TicketFeatureAccess` を再利用・拡張する範囲で実装中判断可能だが、silent broad grant になる場合は escalate する。

---

<!-- event: state_changed author: intake at: 2026-06-09T10:20:48Z from: planning to: ready reason: planning_ready field: state -->

## State changed

Intake が既存 Ticket の要件を確認し、Orchestrator が routing できる状態と判断したため planning から ready に遷移します。実装はまだ開始しません。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T10:31:11Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
