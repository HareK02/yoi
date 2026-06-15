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

<!-- event: implementation_report author: hare at: 2026-06-15T14:20:26Z -->

## Implementation report

### E2E measurements (real `target/debug/yoi` + PTY)

| Scenario | Before | After | Budget | Notes |
| --- | ---: | ---: | ---: | --- |
| Fixture first visible panel render (`panel_full_ready_has_separate_startup_budget`) | 20.342 ms | 20.384 ms | 1500 ms | Warm fixture did not reproduce 7 s latency; after path keeps first draw separate from reload work. |
| Fixture full-ready rows (`panel_full_ready_has_separate_startup_budget`) | 120.576 ms | 120.552 ms | 5000 ms | Separate metric; fixture full-ready remains well below budget after the first-frame deferral. |
| Held-reload ordering (`panel_first_visible_render_arrives_before_background_reload`) | `background_task_started` before `panel_ready` | `panel_ready` before held `background_task_started`; first visible 20.396 ms | 1500 ms | Guarantees initial visible frame is not blocked by reload/observation. |
| Pre-change ad-hoc Panel run from existing fixture artifact | `background_task_started@76 ms`, `panel_ready@80 ms`, full rows `@182 ms` | latest held/full tests above | n/a | Used to identify wait ordering; fixture still did not reproduce live 7 s. |

### Wait points identified

- Synchronous before first draw: CLI/process startup, `run_panel` workspace/cwd setup, `load_app` construction of an empty/loading `WorkspacePanelViewModel`, raw-mode/bracketed-paste/alternate-screen/mouse setup, and the first `terminal.draw`.
- Previously scheduled before first draw: initial Panel reload/observation task (`PendingReload::start(Ensure { ... })`), which can scan Tickets/Pods/orchestrator state and perform socket/status probing before fixture rows are fully ready.
- Background/full-ready after first draw: `load_multi_pod_snapshot`, Ticket list/detail loading, Pod metadata/status checks, orchestrator lifecycle observation, row selection/re-render, and background diagnostics.

### Changes

- Added PTY E2E startup coverage for `yoi panel`:
  - `panel_first_visible_render_arrives_before_background_reload` asserts `panel_ready` arrives within 1500 ms and before the held reload task starts.
  - `panel_full_ready_has_separate_startup_budget` asserts first visible render within 1500 ms and full fixture rows within 5 s as a separate metric.
- Deferred the initial Panel reload start until after the first loading frame is drawn, preserving later background reload correctness.
- Kept Panel/terminal mouse capture to SGR + normal tracking (`?1006h` + `?1000h`) and avoided drag-capture (`?1002h`/`?1003h`) so existing PTY tests can confirm no drag-capture regression.

### Guaranteed scope / residual gaps

- Guaranteed by E2E fixture: real binary, PTY, first visible frame budget, held-reload ordering, separate full-ready row budget, no provider/network/secret dependency.
- Residual live/manual gap: the reported ~7 s live Panel startup did not reproduce in this fixture. This change prevents initial reload/observation from blocking or contending with the first visible frame, but live-terminal confirmation is still needed if the remaining cause is workspace-specific (for example a large real Ticket/Pod set or slow live socket/status probe).


---

<!-- event: review author: yoi-reviewer-00001KV5MRH6D at: 2026-06-15T14:31:11Z status: approve -->

## Review: approve

Review result: approve

Evidence checked:
- Reviewed child worktree diff `4772c4d6..9bad2745`.
- Reviewed changed files:
  - `.yoi/tickets/00001KV5MRH6D/item.md`
  - `.yoi/tickets/00001KV5MRH6D/thread.md`
  - `crates/tui/src/multi_pod.rs`
  - `crates/tui/src/single_pod.rs`
  - `tests/e2e/tests/panel.rs`
- Worktree was clean and at expected branch/commit.

Acceptance review:
- E2E uses real `yoi panel` process under PTY fixture, not unit-only coverage.
- First visible render is explicitly measured and asserted via `panel_first_visible_render_arrives_before_background_reload` with a 1500ms budget.
- Full-ready/background work is separately measured via `panel_full_ready_has_separate_startup_budget` with its own budget and ready-row assertion.
- The implementation defers the initial background reload until after the first loading frame is queued/drawn; it does not fake Ticket/Pod/Orchestrator state.
- The tests use a held reload barrier to prove first visible rendering happens before background reload unblocks.
- Background reload correctness is preserved by waiting for rows/actions after the barrier.
- The reported before/after evidence and thread update correctly explain that fixture E2E did not reproduce the live ~7s delay and that live/manual confirmation remains a residual gap.
- Existing no-provider/no-network style E2E isolation is preserved; no provider/secret/network dependency was introduced.
- The mouse capture adjustment avoids drag/full-motion capture modes while preserving SGR normal mouse tracking, matching recent Panel mouse invariants.
- No Ticket workflow, Pod restore/spawn authority, Orchestrator queue, scheduler, or lease semantics were changed.

Validation performed by reviewer:
- Passed: `git diff --check 4772c4d6..9bad2745`
- Passed: `cargo fmt --check`
- Worktree stayed clean.

Validation not run by reviewer:
- E2E/cargo test commands were not rerun because review scope was read-only and they write build artifacts. Coder-reported validation was inspected as evidence.

Conclusion:
- Approved. No blocker remains.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-15T14:31:28Z -->

## Implementation report

Integration report:

Merged implementation/evidence branch into orchestration branch:
- branch: `impl/00001KV5MRH6D-panel-startup-latency`
- implementation commit: `9bad2745 fix: measure and defer panel startup reload`
- merge commit: `6f99ebed merge: panel startup latency e2e`

Reviewer result:
- `approve` recorded from `yoi-reviewer-00001KV5MRH6D`.

Validation in Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration`:
- Passed: `cargo fmt --check`
- Passed: `cargo test -p yoi-e2e --features e2e --no-run`
- Passed: `cargo test -p yoi-e2e --features e2e`
  - yoi-e2e lib: 1 passed
  - panel E2E: 5 passed
  - rewind E2E: 1 passed
- Passed: `cargo check -p yoi-e2e -p yoi -p tui`
- Passed: `cargo test -p tui single_pod_mouse_capture_avoids_drag_and_all_motion_modes`
  - 1 passed
- Passed: `git diff --check`

Evidence outcome:
- First visible render is asserted within 1500ms through real `yoi panel` PTY fixture.
- Full-ready/background row availability is asserted separately within 5s.
- Initial reload/observation is deferred until after first loading frame; state/diagnostics are not faked or skipped.
- Fixture did not reproduce the live ~7s startup delay, so live/manual confirmation remains a documented gap.

Cleanup planned:
- Stop related coder/reviewer Pods.
- Remove only child implementation worktree/branch for this Ticket.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-15T14:31:28Z from: inprogress to: done reason: merged_validated field: state -->

## State changed

Reviewer approved, implementation/evidence branch merged into the orchestration branch, and E2E-focused validation passed in the Orchestrator worktree. Marking Ticket done in the orchestration branch.

---
