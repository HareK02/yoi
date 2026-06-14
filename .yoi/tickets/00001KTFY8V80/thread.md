<!-- event: create author: LocalTicketBackend at: 2026-06-07T02:23:28Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: comment author: ticket-intake at: 2026-06-14T14:13:35Z -->

## Comment

## Intake refinement

既存 Ticket `00001KTFY8V80` を確認した。新規 duplicate Ticket は作成しない。

### Readiness

- readiness: implementation_ready
- risk_flags: [prompt-context, persistence, workflow-state, compaction]

この Ticket は、active workflow を compaction / rehydration 後も継続可能にする concrete work item として十分に bounded されている。実装戦術の調査余地は残るが、Orchestrator が implementation routing できる要件・受け入れ条件・検証観点は揃っている。

### Binding decisions / invariants

- active workflow の進行中状態を、history に残らない transient context 注入だけで復元してはならない。
- compaction / restore 後に「どの workflow が継続中か」「どの手順段階・義務が残っているか」をモデルが説明可能でなければならない。
- workflow state の復元は、prompt context 加工原則に反しない形で durable source から再構成する。
- missing / corrupt / obsolete workflow state は fail-closed または bounded diagnostic として扱い、silently stale instructions を実行しない。
- Ticket / Pod history / workflow record / compaction output の authority boundary を混同しない。

### Implementation latitude

- workflow state の永続化先・schema・snapshot 粒度は、既存 Pod/session/compaction architecture に合わせて選んでよい。
- active workflow body を invocation-time snapshot として保持するか、rehydration 時に最新 resource を参照するかは、実装時に明示的に決定し、互換性・安全性の理由をコードまたは docs / Ticket 報告に残す。
- UI/diagnostic 表示の具体的な文言や internal field 名は、既存設計に沿って調整してよい。

### Escalation conditions

- workflow snapshot vs latest body の選択が authority boundary または backward compatibility を大きく変える場合。
- compaction が workflow obligations を再現するために hidden context injection を必要としそうな場合。
- persisted workflow state の migration / compatibility 方針が既存 records を破壊する場合。
- implementation が Ticket lifecycle / Orchestrator queue semantics / workflow invocation semantics を広げる必要を見つけた場合。

### Related context checked

- closed `00001KTG3AZQ8` / `00001KTG3BX0R` は Orchestrator routing / merge completion の完了済み関連文脈であり、本 Ticket の duplicate ではない。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-14T14:13:43Z -->

## Intake summary

既存 Ticket `00001KTFY8V80` を精査し、duplicate は作成しない方針で refinement を記録した。対象は active Workflow invocation/state/obligations を durable state/history と compaction/rehydration 経路に載せ、compaction 後も `/multi-agent-workflow` / `/worktree-workflow` などの active obligations を traceable に継続できるようにする実装 work item。readiness は implementation_ready。risk flags は prompt-context / persistence / workflow-state / compaction。Orchestrator は implementation routing 可能だが、snapshot vs latest workflow body の選択、hidden context injection 回避、missing/corrupt persisted state の fail-closed diagnostic、Ticket/Pod/history/workflow authority boundary を reviewer focus に含める。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-14T14:13:43Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement が完了し、要件・受け入れ条件・binding invariants・escalation conditions が Ticket thread に記録されたため `planning -> ready` にします。実装 side effects は Orchestrator routing 後に行います。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-14T15:23:07Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
