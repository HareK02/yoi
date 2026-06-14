<!-- event: create author: LocalTicketBackend at: 2026-06-13T10:54:31Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-14T14:12:11Z -->

## Intake summary

既存 Ticket 00001KV09WYC6 の body/thread/artifacts を確認した。要件、受け入れ条件、binding decisions / invariants、implementation latitude、escalation conditions、validation が揃っており、readiness は `implementation_ready` と判断できる。未解決の blocking open question はない。risk_flags は `panel-ux`, `local-role-session-registry`, `pod-session-state`。次は Orchestrator が routing し、実装時は Panel の selection/keyboard semantics、one-active-claim-per-Ticket invariant、pre-Ticket Intake の誤関連付け回避、registry schema migration 不要の維持を重点確認する。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-14T14:12:11Z from: planning to: ready reason: planning_ready field: state -->

## State changed

Intake 確認により、既存 Ticket の要件は実装 routing 可能な状態と判断した。実装 side effect は Orchestrator の queue/routing flow に委ねる。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-14T15:23:27Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-14T15:24:40Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- 要件、受け入れ条件、binding decisions / invariants、implementation latitude、escalation conditions、validation が Ticket body/thread に揃っている。
- Panel 表示改善に限定された concrete work item であり、registry/schema redesign や自動 spawn/polling は非要件として明確。

Evidence checked:
- Ticket body / thread / artifacts: artifacts なし、Intake summary と `planning -> ready`、Panel `ready -> queued` を確認。
- Ticket relations: blocking relation なし。
- OrchestrationPlan records: 既存 record なし。
- Orchestrator workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、queue commit `d311fe8f` 上。
- Visible Pods: spawned child なし。
- Related code map: `crates/tui/src/workspace_panel.rs`, `crates/tui/src/multi_pod.rs`, `crates/tui/src/role_session_registry.rs`。

IntentPacket:

Intent:
- Workspace Panel の Ticket 表示で、Ticket-associated Intake Pod/session を Ticket row と隣接・関連表示し、open/attach 対象として認識しやすくする。

Binding decisions / invariants:
- local Pod assignment / Pod name / socket / claim state / runtime status を git-tracked Ticket metadata/frontmatter/thread に保存しない。
- automatic polling / automatic Intake spawn を追加しない。
- selected arbitrary Pod direct-send UX を復活させない。
- Ticket と Intake Pod の関係を 1:1 と仮定しない。
- one-active-claim-per-Ticket invariant を維持する。
- 既存 local role/session registry を基本入力とし、新しい durable Ticket schema は導入しない。

Requirements / acceptance criteria:
- Ticket に local Intake claim / related Intake session がある場合、Panel 上で Ticket と隣接・関連表示される。
- subtitle に埋もれず「この Ticket の Intake Pod」と認識できる。
- live / restorable / stale の状態が確認できる。
- 関連 Intake Pod の open/attach 導線が Panel 操作から使える、または既存 open/attach 操作へ明確に誘導される。
- pre-Ticket Intake Pod を特定 Ticket に誤関連付けしない。
- focused test で ViewModel/row ordering または rendering contract を確認する。

Implementation latitude:
- 表示形式は既存 Panel UI に合わせて、隣接 row / child row / inline chip / row group のいずれかを選んでよい。
- `related_pods` / `local_claim` ViewModel 拡張または typed row kind 追加は実装判断。
- subtitle 表示の整理は重複して読みにくくならない範囲で可。

Escalate if:
- Panel selection model / keyboard semantics の大幅変更が必要になる。
- one-active-claim-per-Ticket を崩さないと目的を満たせない。
- pre-Ticket Intake と existing-Ticket Intake の表示分類が曖昧になり、誤関連付けのリスクがある。
- Registry schema migration が必要になる。

Validation:
- `cargo test -p tui workspace_panel --lib`
- 関連箇所を触る場合 `cargo test -p tui multi_pod --lib` / `cargo test -p tui role_session_registry --lib`
- `cargo fmt --check`
- `git diff --check`

Current code map:
- `crates/tui/src/workspace_panel.rs`
- `crates/tui/src/multi_pod.rs`
- `crates/tui/src/role_session_registry.rs`

Critical risks / reviewer focus:
- Ticket metadata 汚染の有無。
- one-active-claim invariant。
- pre-Ticket Intake の誤関連付け。
- selection/open semantics の不要な変更。
- bounded row rendering / large list readability。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-14T15:24:58Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。blocking relation / orchestration-plan blocker はなく、Orchestrator workspace は clean。00001KTFY8V80 とは主対象が workflow/compaction と TUI Panel で分かれており、独立 worktree/branch で並行開始可能と判断したため、implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---
