<!-- event: create author: yoi-intake at: 2026-06-15T12:40:33Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-15T13:59:47Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-15T14:01:09Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- Ticket body / thread / relations / OrchestrationPlan / Orchestrator workspace state を確認した。blocking relation はなく、planning に戻す concrete missing information はない。
- Work item は Panel startup latency の measurement / E2E budget / concrete wait-point improvement に限定され、Ticket workflow / Pod authority / Orchestrator queue semantics を変更しない invariant が明確。
- 同時 queued の Plugin resolver work とは source surface が大きく異なるため並行開始可能。

Evidence checked:
- Ticket body/thread: startup wait points、first visible vs full ready distinction、E2E acceptance criteria、binding decisions、escalation conditions、validation を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`425a6c66` 上。
- Visible Pods: implementation child Pod なし。
- Related context: `00001KTFMMZP0` prior non-blocking transition work、`00001KV3BQ7Q3` Panel/TUI E2E behavior evidence work は closed/done context として参照。

IntentPacket:

Intent:
- `yoi panel` startup path を E2E/fixture PTY で計測し、first visible render と background/full-ready wait を分けて可視化し、実ユーザーに効く startup latency を改善・基準化する。

Binding decisions / invariants:
- focused/unit/code review だけで startup latency 改善済み扱いにしない。
- E2E pass と manual/live terminal confirmation を混同しない。
- `first visible render` と `all background work complete` を同一 metric にしない。
- 起動を速く見せるために Ticket / Pod / Orchestrator state authority を偽らない。
- Background reload / observation 完了後は正しい state / diagnostics を反映する。
- Provider/network/secret dependency を導入しない。
- Broad TUI runtime rewrite / scheduler / lease 導入は non-goal。

Requirements / acceptance criteria:
- Real `yoi` binary + PTY path の E2E で startup time を測る。
- 少なくとも initial visible panel/render 到達時間を assert する。
- First visible budget を明示する（提案: <= 1500ms; 実測で妥当でない場合は理由付き調整）。
- Full ready/background reload complete を測るなら別 metric/budget として扱う。
- Before/after 測定結果、major wait point、削減/非同期化/遅延実行した wait、E2E が保証する範囲、live/manual gap を実装報告に記録する。
- Existing E2E fixture-local HOME/XDG/runtime/workspace isolation and no-provider/no-network を維持する。

Implementation latitude:
- E2E-only observer / timing marker / PTY output marker / structured diagnostic event の選択は実装判断。
- Wait point breakdown の粒度は実装判断だが報告で説明可能にする。
- If fixture does not reproduce 7s latency, record what is guaranteed and what remains live/manual gap。
- Startup improvement can use existing PendingReload / background observation / loading state。

Escalate if:
- 7s latency is live/workspace-specific and not fixture reproducible。
- Stable timing test requires broader harness redesign。
- Wait cause is storage corruption / huge metadata / provider / filesystem stalls outside fixture。
- Fix requires Pod authority / Ticket workflow / Orchestrator lifecycle semantic changes。
- Meaningful latency threshold cannot be made non-flaky。

Validation:
- `cargo test -p yoi-e2e --features e2e` or narrow scenario plus full if feasible。
- `cargo test -p yoi-e2e --features e2e --no-run`。
- `cargo fmt --check`。
- `git diff --check`。
- Depending changes: `cargo check -p yoi-e2e -p yoi -p tui` and focused tui tests。

Critical risks / reviewer focus:
- E2E actually measures first visible render, not just process start。
- first visible vs full ready distinction。
- threshold not overly flaky and not permissive of 7s regression。
- no authority/state falsification for speed。
- fixture isolation / no-provider / no-network。
- before/after evidence and residual live gap clarity。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-15T14:01:19Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing decision と accepted implementation/evidence plan を記録済み。blocking relation / unresolved OrchestrationPlan blocker はなく、Panel startup latency work は同時に開始する Plugin resolver work と主対象が異なるため、implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---
