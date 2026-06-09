<!-- event: create author: LocalTicketBackend at: 2026-06-09T10:17:32Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-09T10:24:23Z -->

## Intake summary

Ticket 20260609-101732-001 は implementation_ready。目的は Orchestrator / multi-agent workflow guidance に parallel capacity policy を追加し、独立した queued work が複数あり capacity が空いている場合に、安全確認後は 1 件ずつ待たず並列受理・起動を優先するよう明文化すること。非目標は automatic background scheduler、unqueued Ticket の開始、OrchestrationPlan store/runtime scheduling の変更。既存安全不変条件（queued -> inprogress 前の副作用禁止、blind spawn 禁止、worktree/write-scope 分離、relation blocker 尊重、reviewer read-only 原則）は維持する。関連 Ticket 20260608-061235-001 は broader starvation-prevention/planning work であり、本 Ticket は workflow guidance に限定した non-blocking follow-up として扱う。risk_flags: [orchestration-policy, workflow-guidance, parallelism, authority-boundary, scope-isolation]。未解決の blocking question はない。

---

<!-- event: state_changed author: intake at: 2026-06-09T10:24:23Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake により要件・受け入れ条件・非目標・安全不変条件・関連 work を確認した。実装は prompt/workflow resource の focused update と必要な snapshot/test 更新に限定でき、Orchestrator が routing 可能な状態。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T10:31:16Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
