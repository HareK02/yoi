<!-- event: create author: ticket-intake at: 2026-06-13T16:29:25Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-13T16:29:41Z -->

## Intake summary

ユーザー依頼に基づき、Panel Orchestrator の自動作成 orchestration branch 名を `.yoi/ticket.config.toml` の typed config として設定可能にする concrete Ticket を作成した。設定なしでは既存 default `orchestration/<workspace-orchestrator-pod-name>` を維持し、invalid / mismatched worktree は破壊的修復せず diagnostic で止める方針。blocking open question はない。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-13T16:29:41Z from: planning to: ready reason: planning_ready field: state -->

## State changed

Intake 済み。要件・受け入れ条件・binding invariants・validation が揃っており、Orchestrator routing 可能。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-13T16:33:27Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
