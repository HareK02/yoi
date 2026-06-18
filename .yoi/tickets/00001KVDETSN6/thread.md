<!-- event: create author: "yoi ticket" at: 2026-06-18T13:30:51Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-18T13:55:08Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-18T13:58:25Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- 主目的は Panel startup latency の測定基準を、first frame / 単一 Ticket row readiness ではなく、ユーザー目線の dashboard content ready に揃えること。
- 要件、非目標、validation、escalation 条件が実装可能な粒度で揃っており、残る不確実性は Panel/E2E/TUI harness 近傍の bounded implementation investigation に閉じる。
- `depends_on` / incoming `blocks` の未解決 blocker は見当たらない。関連 Ticket は完了済みまたは context link として扱える。
- 既存 in-progress Ticket `00001KV5W3PJ3` は Plugin permission grants 領域で、今回の主作業面は Panel startup/E2E/TUI harness 側のため、別 worktree での並行実装は conflict risk が低い。

Evidence checked:
- Ticket `00001KVDETSN6` body/thread/artifacts via `TicketShow`。
- `TicketRelationQuery(00001KVDETSN6)` の relation metadata: blocking acceptance blocker はなし。
- `TicketOrchestrationPlanQuery(00001KVDETSN6)`: 既存 plan record はなし。今回 `accepted_plan` を記録済み。
- 関連 Ticket `00001KV62PF32`, `00001KV5MRH6D`, `00001KV5D7MG5` の状態: done/closed context として確認。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration` の git state と既存 worktree/branch: matching implementation branch/worktree はなし。
- Code map: `panel_ready`, `rows_rendered`, startup latency / fixture readiness 周辺の既存実装・テスト候補を grep で確認。
- Visible Pods: 既存 Coder `yoi-coder-00001KV5W3PJ3` は別 Ticket 用。今回の worktree / branch / scope を分離できる。

IntentPacket:

Intent:
- Panel startup latency の主な evidence を、ユーザーが dashboard として意味ある内容を見られる状態へ合わせる。
- live-like fixture と expected dashboard content snapshot を使い、Ticket/Pod/claim など代表 dashboard data が描画・利用可能になるまでを測定できるようにする。
- 測定結果から遅延源を分解し、必要な範囲で startup/readiness 改善を行う。

Binding decisions / invariants:
- `panel_ready` や first frame は主 UX metric にしない。必要なら補助 metric として残す。
- `rows_rendered`/単一 Ticket row readiness だけで dashboard content ready と見なさない。
- E2E/fixture はユーザーに見える dashboard content を代表すること。空画面や trivial row だけの readiness は不可。
- Panel は scheduler/backend ではなく local-file-first view である、という既存設計を変えない。
- Mouse/input semantics や Panel queue/close/review workflow semantics をこの Ticket で広げない。
- root/original workspace は操作せず、Orchestrator worktree から作成した child implementation worktree だけで実装する。

Requirements / acceptance criteria:
- Dashboard content ready を測る fixture/test harness または equivalent な計測 surface が追加される。
- Expected dashboard content snapshot / assertion があり、ユーザーに意味のある複数種の dashboard rows/data が ready 条件に含まれる。
- Startup latency 出力に first frame と dashboard content ready の違い、または slow-source breakdown が分かる evidence がある。
- 既存 Panel startup regression test / benchmark 相当が新しい基準に合わせて更新される。
- 改善実装を入れる場合は、semantic shortcut ではなく実際の readiness path の遅延削減であること。

Implementation latitude:
- 既存 Panel test/fixture structure を調査し、最小の fixture/harness 拡張で dashboard content ready を表現してよい。
- Metric 名、structured output field 名、test helper の分割は既存コードに合わせてよい。
- 遅延改善は、測定で見えた局所的な loading/readiness bottleneck に限定してよい。

Escalate if:
- Panel architecture を scheduler/backend 化する必要が出る。
- Dashboard ready の定義に product/UX 判断が必要な未記録の分岐がある。
- Terminal/PTY 実 E2E の新規大規模設計が必要になる。
- Existing Ticket lifecycle / queue semantics を変更しないと達成できない。

Validation:
- Focused Panel startup / fixture / snapshot tests。
- Relevant `cargo test` / `cargo check`。
- `cargo fmt --check`。
- `git diff --check`。
- `nix build .#yoi` は dependency/Cargo.lock/package-source-filter 変更時のみ。

Current code map:
- Panel startup metric / rows readiness / test fixtures around matches for `panel_ready`, `rows_rendered`, `startup latency`, dashboard fixture readiness.
- Likely crates: `crates/tui` and related integration/E2E harness files.
- Avoid unrelated Plugin permission grant worktree and root/original workspace.

Critical risks / reviewer focus:
- Metric rename/additionが実際の UX readiness を測らず名前だけ変えていないか。
- Fixture が live-like で、Ticket/Pod/claim など dashboard content を代表しているか。
- Slow-source breakdown が regression triage に使える bounded output か。
- Startup performance 改善が semantics を壊す shortcut ではないか。
- Existing Panel behavior / queue semantics / row selection semantics を accidental に変更していないか。

Next action:
- `queued -> inprogress` を記録し、Orchestrator worktree の tracked Ticket records を commit してから、専用 implementation worktree を作成し Coder Pod を narrow write scope で起動する。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-18T13:58:33Z from: queued to: inprogress reason: orchestrator_acceptance_dashboard_content_ready field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。Ticket body/thread、relations、OrchestrationPlan、関連 Ticket、Orchestrator worktree、visible Pods、既存 branch/worktree を確認し、blocking relation / conflict / missing planning decision は見つからなかった。Panel startup dashboard-content-ready work は既存 Plugin permission grants work と主対象が異なり、別 worktree/branch/scope で並行可能なため、implementation side effects の前に acceptance を記録する。

---
