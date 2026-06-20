<!-- event: create author: LocalTicketBackend at: 2026-06-20T06:55:49Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-20T06:55:57Z -->

## Intake summary

ユーザー合意に基づき、Dashboard / Console 呼称導入、`yoi panel` Dashboard 境界、単一 Pod Console 境界、`multi_pod.rs` の Dashboard module への移設・分割、docs/help/tests 更新を 1 つの concrete implementation Ticket として整理した。readiness は `implementation_ready`。risk flags は stop gate ではなく reviewer focus として扱う。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-20T06:55:57Z from: planning to: ready reason: planning_ready field: state -->

## State changed

要件・受け入れ条件・binding decisions・validation が揃っており、Orchestrator が実装 routing 可能。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T08:30:58Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T08:31:27Z -->

## Decision

Routing decision: waiting_capacity

Panel Queue により routing 対象として確認したが、現在 `00001KVHR3WS6` が `inprogress` で MCP tool registration の Coder/Reviewer 作業中のため、この Dashboard / Console refactor Ticket は実装開始せず queued のまま保持する。

Evidence:
- Ticket body / relations を確認。未解決 blocker relation はない。
- Orchestrator worktree は clean。
- inprogress は `00001KVHR3WS6` のみ。
- matching branch/worktree は存在しない。
- 現在 active child Pods は `yoi-coder-00001KVHR3WS6` と `yoi-reviewer-00001KVHR3WS6-r1`。

Next:
- 現在の `00001KVHR3WS6` 作業が完了して implementation capacity が空いた後、改めて reroute する。

---
