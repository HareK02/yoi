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

<!-- event: decision author: yoi-orchestrator at: 2026-06-14T15:24:40Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- 要件、受け入れ条件、binding invariants、implementation latitude、escalation conditions が Ticket body/thread に揃っている。
- active Workflow invocation/state/obligations を durable history/state と compaction/rehydration 経路に載せる目的は concrete で、残る不確実性は既存 Pod/session/compaction architecture 内の実装戦術選択に閉じている。

Evidence checked:
- Ticket body / thread / artifacts: artifacts なし、Intake refinement と `planning -> ready`、Panel `ready -> queued` を確認。
- Ticket relations: blocking relation なし。
- OrchestrationPlan records: 既存 record なし。
- Orchestrator workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、queue commit `d311fe8f` 上。
- Visible Pods: spawned child なし。
- Bounded code map: workflow / compaction 関連は `crates/pod/src/compact/*`, `crates/pod/src/workflow/*`, `crates/pod/src/prompt/*`, `crates/session-store/src/*`, `crates/protocol/src/lib.rs`, `resources/workflows/*` が候補。

IntentPacket:

Intent:
- compaction を跨ぐ長時間 workflow-governed task で、active workflow と残る operational obligations が失われないようにする。

Binding decisions / invariants:
- Workflow instructions を、history/state に残らない turn-local transient context だけを根拠に model context へ注入しない。
- post-compaction context は「available workflow」と「この task で active な workflow obligations」を区別する。
- missing / corrupt / obsolete active workflow state は silent stale instruction ではなく fail-closed または bounded diagnostic にする。
- Ticket / Pod history / workflow record / compaction output の authority boundary を混同しない。
- active workflow state は workflow-governed task の完了または explicit cancellation で clear / completed にできる必要がある。

Requirements / acceptance criteria:
- active workflow の slug、invocation source/time、task/scope、active/completed、current obligations/checkpoints を durable typed history/state として表現する。
- compaction が active workflow state を明示的に carry forward する。
- rehydration が durable source から active workflow guidance を復元できる。
- snapshot vs latest workflow body の選択を実装報告または docs/code に明示する。
- focused coverage に、review delegation と merge/close handling の間で compaction が起きる worktree/multi-agent style flow を含める。

Implementation latitude:
- 永続化先、schema、snapshot 粒度、diagnostic 表現は既存 Pod/session/compaction architecture に合わせて選んでよい。
- local tactic 調査は coder に委ねるが、authority boundary を広げる必要があれば escalate する。

Escalate if:
- workflow snapshot vs latest body の選択が authority boundary や backward compatibility を大きく変える。
- compaction 復元が hidden context injection を必要としそうになる。
- persisted workflow state migration / compatibility が既存 records を破壊しそうになる。
- Ticket lifecycle / Orchestrator queue semantics / workflow invocation semantics を広げる必要が出る。

Validation:
- 変更箇所に応じて `cargo test` / `cargo check` の focused subset。
- 少なくとも workflow/compaction 関連 unit coverage、`cargo fmt --check`、`git diff --check`。

Current code map:
- Primary candidates: `crates/pod/src/compact/*`, `crates/pod/src/workflow/*`, `crates/pod/src/prompt/*`, `crates/session-store/src/*`, `crates/protocol/src/lib.rs`。
- Workflow resources: `resources/workflows/*`。

Critical risks / reviewer focus:
- hidden context injection 回避。
- active vs advertised workflow の明確な区別。
- stale workflow obligations の漏れ込み防止。
- persisted state の compatibility / corrupt-state diagnostics。
- compaction 後の traceability と test coverage。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-14T15:24:58Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。blocking relation / orchestration-plan blocker はなく、Orchestrator workspace は clean。implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---
