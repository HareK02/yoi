<!-- event: create author: ticket-intake at: 2026-06-23T07:02:07Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-23T08:41:00Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-23T12:42:28Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Ticket 自体は `implementation_ready` と判断できる。目的、binding decisions / invariants、acceptance criteria、validation、escalation conditions は揃っており、`tui-keybinding` / `pod-lifecycle` risk は reviewer focus として扱える。
- ただし routing 時点の merge target `/home/hare/Projects/yoi` に、この Ticket の対象ではない `crates/tui/src/dashboard/mod.rs` と `crates/tui/src/dashboard/tests.rs` の未コミット変更がある。workflow の dirty-state gate に従い、この状態では implementation worktree 作成 / Coder Pod 起動 / merge へ進めない。
- 既存 worktree entry `.worktree/00001KVSMJJNV-paused-ctrlx-cancel` は `git worktree list` 上で prunable と表示され、gitdir が存在しないため、再利用には cleanup/recreate 判断が必要。

Evidence checked:
- Ticket body / thread / artifacts: TicketShow で body、queued event、artifacts なしを確認。
- Ticket relations: TicketRelationQuery で blocker relation なし。
- Orchestration plan: TicketOrchestrationPlanQuery で accepted-plan / conflict / waiting record なし。
- Queue state: queued Ticket はこの 1 件のみ。ready / inprogress は 0 件。
- Workspace/worktree/Pod state: `/home/hare/Projects/yoi` は `develop...origin/develop [ahead 48]` かつ dashboard files に dirty change あり。`.worktree/orchestration` は clean。visible Pods は peer `yoi` idle と self `yoi-orchestrator` running、spawned child なし。

Next action:
- merge target の未コミット dashboard 変更を所有者が commit / revert / 明示的に退避許可した後、この Ticket を再 routing する。
- その時点で blocker がなければ `queued -> inprogress` を記録してから、worktree を clean に recreate し、Coder/Reviewer loop に渡す。

Escalate if:
- dashboard 変更がこの Ticket と同時に保持すべき作業なら、別 Ticket/branch として扱うか、今回の implementation worktree/merge 計画に含めるかを人間が決める。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-23T13:21:02Z -->

## Decision

Routing correction:

直前の `blocked_by_dependency_or_missing_authority` 判断は取り下げる。merge target `/home/hare/Projects/yoi` の dashboard files dirty は最終 merge 時の確認事項だが、この Ticket の implementation acceptance / implementation worktree 作成を止める gate として扱うのは過剰だった。

Corrected routing decision: implementation_ready

Evidence checked:
- Ticket body / thread: 目的、requirements、acceptance criteria、binding decisions / invariants、implementation latitude、escalation conditions、validation が揃っている。
- TicketRelationQuery: blocker relation なし。
- TicketOrchestrationPlanQuery: accepted-plan / conflict / blocker / waiting-capacity record なし。
- Queue/capacity: queued Ticket はこの 1 件。visible spawned child はなし。
- Worktree state: orchestration worktree は Ticket 記録変更のみ。既存 `.worktree/00001KVSMJJNV-paused-ctrlx-cancel` は prunable なので cleanup/recreate 対象。

Next action:
- `queued -> inprogress` を記録する。
- stale/prunable implementation worktree entry を掃除して、`.worktree/00001KVSMJJNV-paused-ctrlx-cancel` を作り直す。
- Coder Pod に IntentPacket を渡して実装を開始する。

Merge note:
- merge target 側の unrelated dashboard dirty は final merge 前に再確認し、必要なら人間へ確認する。implementation worktree 内の作業開始 blocker にはしない。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-23T13:21:07Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Orchestrator acceptance: Ticket evidence、relation blockers、orchestration plan、worktree/capacity 状態を再確認し、implementation_ready と判断した。implementation side effect 前に queued -> inprogress を記録する。

Acceptance basis:
- blocker relation なし。
- orchestration plan blocker/conflict なし。
- queued Ticket はこの 1 件で child capacity あり。
- risk flags は `tui-keybinding` / `pod-lifecycle` として IntentPacket の reviewer focus に反映する。

Next:
- implementation worktree を clean に作成し、Coder Pod に routing する。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-23T13:21:47Z -->

## Plan

IntentPacket for implementation

Intent:
- TUI Console で Pod が `Paused` のとき、`Ctrl+X` を TUI shutdown ではなく paused/interrupted turn の cancel として扱えるようにする。
- Paused cancel 後は resume 可能な paused turn を残さず、次の新規入力を通常の新規 turn として受けられる状態にする。

Binding decisions / invariants:
- `Ctrl+C` の Running -> Pause semantics は変更しない。
- `Ctrl+C` の Idle/Paused 2-tap TUI quit guard はこの Ticket では置き換えない。
- `Running` 中の `Ctrl+X -> Method::Cancel` は維持する。
- `Idle` 中の `Ctrl+X` は、必要な明示理由がなければ現状の shutdown を維持する。
- in-flight input injection / history append policy は扱わない。
- provider stream を直接 mutate しない。
- cancel は hidden context/history mutation ではなく、既存の typed `Method::Cancel` semantics またはそれに準じる明示的 lifecycle 操作で表現する。

Requirements / acceptance criteria:
- `Ctrl+C` で Running turn を Pause した後、Console 上で `Ctrl+X` を押すと TUI exit / Pod shutdown ではなく turn cancel が実行される。
- Paused cancel 後、`Enter` で前の turn が resume されない。
- Paused cancel 後、新規入力は通常の新規 turn として扱われる。
- Running cancel と Idle `Ctrl+X` に意図しない regression がない。
- focused tests が Paused 状態の `Ctrl+X` key handling と Pod/controller 側の cancel semantics をカバーする。

Implementation latitude:
- `Paused` 中の `Ctrl+X` を `Method::Cancel` に変えるだけで足りるか、controller/worker 側に Paused cancel handling を追加するかは実装調査で判断してよい。
- UI hint の文言や配置は実装者に任せてよい。
- 既存 test の修正・分割は許容する。

Escalate if:
- `Method::Cancel` を Paused 状態に拡張すると session/history cleanup や orphan tool-result closure の新しい設計判断が必要になる。
- `Ctrl+X` の Idle 挙動変更が必要になる。
- provider stream / history mutation / lifecycle authority の境界を変える必要が出る。

Validation:
- focused: `cargo test -p tui pause` または該当 TUI key handling tests。
- focused: `cargo test -p pod cancel` または controller paused cancel tests。
- `cargo fmt --check`。
- 必要に応じて `cargo check -q`。

Current code map:
- 主対象: `crates/tui/src/console/mod.rs`。
- 関連: `crates/tui/src/ui.rs`, `crates/protocol/src/lib.rs`, `crates/pod/src/controller.rs`。
- 既存事実: `Ctrl+X` は Running のみ `Method::Cancel`、Paused/Idle は shutdown 側。controller の非-running Cancel は `NotRunning` 経路がある可能性。

Critical risks / reviewer focus:
- Paused と Idle の key handling を混同して shutdown behavior を壊していないか。
- Running cancel の既存 behavior / queued input clear に regression がないか。
- Paused cancel 後に resume 可能な interrupted state が残らないか。
- Cancel semantics が hidden history/context mutation になっていないか。
- tests が実際に app status を Paused に更新した状態をカバーしているか。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T13:37:51Z -->

## Implementation report

Implementation report

Worktree / branch:
- `/home/hare/Projects/yoi/.worktree/00001KVSMJJNV-paused-ctrlx-cancel`
- `work/00001KVSMJJNV-paused-ctrlx-cancel`

Commit:
- `90b1a1fc tui: cancel paused turns with ctrl-x`

Implemented changes:
- `crates/tui/src/console/mod.rs`: `Ctrl+X` key handling を Paused 状態でも cancel path に乗せ、Idle の shutdown behavior と分離。
- `crates/tui/src/ui.rs`: Paused UI hint を cancel 操作に合わせて更新。
- `crates/pod/src/controller.rs` / `crates/pod/src/pod.rs`: Paused 状態の `Method::Cancel` を `NotRunning` ではなく paused turn cancel として処理し、resume 可能な interrupted state を破棄して次入力を fresh run として扱う経路を追加。
- `crates/pod/tests/controller_test.rs`: Paused cancel 後に resume されず、次入力が fresh run になることを確認する focused test を追加。

Validation run by Orchestrator:
- `cargo test -p tui ctrl_x -- --nocapture`
- `cargo test -p pod paused_cancel_abandons_resume_and_next_input_is_fresh_run -- --nocapture`
- `git diff --check`
- `cargo fmt --check`

Next:
- Reviewer Pod に recorded IntentPacket と commit diff を基準に review を依頼する。

---
