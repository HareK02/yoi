<!-- event: create author: ticket-intake at: 2026-06-15T06:32:21Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-15T06:32:36Z -->

## Intake summary

ユーザー承認に基づき、Panel Ticket 2行 row と Ticket-associated Intake Pod row の視覚階層改善を concrete Ticket として作成した。既存 Ticket `00001KV12W2RT` / `00001KV09WYC6` の follow-up であり、目的は visual hierarchy / readability 改善に限定される。Ticket lifecycle、relation gate semantics、local role/session registry、claim semantics、automatic launch/spawn policy は変更しない。readiness: implementation_ready。risk_flags: [panel-ux, tui-layout, accessibility, row-selection]。blocking open questions はない。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-15T06:32:36Z from: planning to: ready reason: planning_ready field: state -->

## State changed

Intake で要件・受け入れ条件・binding decisions / invariants・implementation latitude・escalation conditions・validation が整理され、ユーザー承認済み。Orchestrator が routing 可能な ready 状態にする。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-15T06:37:02Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-15T06:39:02Z -->

## Decision

Routing decision: implementation_ready_but_waiting_conflict_capacity

Reason:
- Ticket body / thread / relations / OrchestrationPlan / Orchestrator workspace state を確認した。要件・受け入れ条件・invariants は実装可能な粒度で揃っており、planning に戻す concrete missing information はない。
- ただし同時に `00001KV4ZDMV1` を Panel composer key handling work として accept する。両者は `crates/tui/src/multi_pod.rs` / Panel rendering・input surface に触れる可能性があり、並行すると row rendering / selected row / action identity regression の確認が曖昧になる。
- `00001KV4YAAVY` は single-Pod View selection で並行開始するため、現時点では Panel visual hierarchy work を追加 spawn せず、先行 Panel composer work の outcome を待つ。

Evidence checked:
- Ticket body/thread: visual hierarchy/readability requirements、accessibility、row-selection invariants、validation を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`f0de8413` 上。
- Visible Pods: implementation child Pod なし。これから `00001KV4YAAVY` と `00001KV4ZDMV1` を acceptance 予定。

Next action:
- `00001KV4ZDMV1` の implementation/review/integration 後、Panel rendering surface を再確認し、unblocked なら `queued -> inprogress` acceptance へ進む。
- planning return ではなく queued のまま waiting とする。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-15T06:52:09Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Prior waiting reason は `00001KV4ZDMV1` と Panel rendering/input surface が重なる conflict/capacity だった。
- `00001KV4ZDMV1` は reviewer approve、orchestration branch merge、focused validation、Ticket `done` まで完了したため、prior conflict は解消した。
- Ticket body / thread / relations / orchestration plan / current Orchestrator workspace を再確認した。blocking relation はなく、planning に戻す concrete missing information はない。
- 現在 active な `00001KV4YAAVY` は single-Pod View Item text selection/copy であり、Panel Ticket/Intake visual hierarchy と主対象が異なるため並行開始可能。

Evidence checked:
- Ticket body/thread: Ticket 2行 row、associated Intake Pod row、selection/group visibility、accessibility、bounded rendering、validation を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: prior `conflicts_with 00001KV4ZDMV1` と waiting_capacity_note を確認。`00001KV4ZDMV1` 完了により blocker は解消。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`1d21aae3` 上。
- Visible Pods: `yoi-coder-00001KV4YAAVY` running。source/logical surface は single-Pod View selection であり、本 Ticket と直接競合しない。

IntentPacket:

Intent:
- Workspace Panel の Ticket 2行 row と Ticket-associated Intake Pod row の visual hierarchy / readability を改善し、親子関係・primary/secondary 情報・選択対象が読み取りやすい表示にする。

Binding decisions / invariants:
- Ticket lifecycle state / relation gate semantics は変更しない。
- persisted `waiting` state や新しい Ticket schema は追加しない。
- local Pod assignment / Pod name / socket / claim state / runtime status を git-tracked Ticket metadata/frontmatter/thread に保存しない。
- automatic polling / automatic Intake spawn は追加しない。
- Ticket と Intake Pod の関係を 1:1 と仮定しない。
- selected arbitrary Pod direct-send UX を復活させない。
- 表示改善のために lifecycle action authority boundary を緩めない。
- 色だけに依存せず、indentation / marker / label 等で minimum relationship を示す。

Requirements / acceptance criteria:
- Ticket 2行 row で primary line と secondary line の強弱が確認できる。
- Ticket-associated Intake Pod row が隣接 Ticket の child/related row として認識できる。
- Intake Pod row が Ticket 本体や別 Ticket に見えない。
- selected Ticket row / selected Intake Pod row の見え方が明確で、操作対象が混同されない。
- `ready`, `planning`, `queued/inprogress`, `done/closed`, `ready+waiting` の Ticket row で readability が維持される。
- Intake Pod `live` / `restorable` / `stale` status が確認できる。
- pre-Ticket Intake Pod を特定 Ticket child row のように誤表示しない。
- mouse click / keyboard selection semantics を壊さない。
- Focused tests で row rendering contract または ViewModel/row ordering/selection contract を確認する。

Implementation latitude:
- dim / bold / color / prefix / indentation / separator / role chip / status chip 等は実装判断。
- `PanelRowKind::Ticket` / `PanelRowKind::TicketIntakePod` rendering 調整可。
- 必要なら shared row style helper を整理可。
- Very small terminal では primary state/title と action safety を優先して bounded rendering する。

Escalate if:
- Panel selection model / keyboard semantics の大幅変更が必要。
- Ticket row と Intake Pod row の action identity が曖昧になり誤操作リスクが出る。
- 色/装飾だけでは accessibility / terminal theme 上の問題を避けられない。
- Row rendering のために local role/session registry や Ticket schema 変更が必要。
- bounded row rendering を維持できず大量 Ticket/Pod で一覧が読みにくくなる。

Validation:
- `cargo test -p tui workspace_panel --lib`
- 関連箇所に応じて `cargo test -p tui multi_pod --lib` / `cargo test -p tui row_hit_testing --lib` / `cargo test -p tui mouse_click --lib`
- `cargo fmt --check`
- `git diff --check`
- 可能なら `yoi panel` / PTY 目視または既存 Panel E2E 更新。

Critical risks / reviewer focus:
- visual hierarchy improves readability without changing action identity。
- Ticket vs Intake child row distinction remains non-color-dependent。
- selection highlight and child grouping remain understandable。
- lifecycle authority and local role/session registry boundaries unchanged。
- bounded rendering/narrow terminal behavior。
- recently merged Alt+Enter and invalid Ticket placeholder behavior not regressed。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-15T06:52:19Z from: queued to: inprogress reason: orchestrator_acceptance_after_conflict_resolution field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。先行 `00001KV4ZDMV1` は merge/validation/done 済みで prior conflict/waiting reason は解消。blocking relation / unresolved orchestration-plan blocker はないため、implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---

<!-- event: implementation_report author: yoi-coder-00001KV4ZPAD3 at: 2026-06-15T07:04:06Z -->

## Implementation report

Implementation report from Coder Pod `yoi-coder-00001KV4ZPAD3`:

Commit:
- `f3b435e7 fix: clarify panel ticket row hierarchy`

Changed files:
- `crates/tui/src/multi_pod.rs`
- `crates/tui/src/workspace_panel.rs`

Visual hierarchy decisions:
- Ticket rows now use a clearer two-line structure:
  - primary line uses selected `▶ ` marker and carries state + title
  - secondary line uses `meta` label and selected `│ ` marker, making id/gate/action information clearly secondary
- Ticket-associated Intake Pod rows now render as adjacent child rows:
  - non-selected child marker: `  └ `
  - selected child marker: `  ▶ `
  - `live` / `restorable` / `stale` remains visible in status column
  - title is `Intake Pod: <name>` without embedding tree glyphs in the ViewModel title
- Selected Intake Pod status line now says `selected Intake Pod ...` instead of generic selected Pod wording.
- No Ticket lifecycle, relation gate, action authority, persisted schema, polling, or spawn semantics were changed.

Focused tests added/updated:
- Ticket row rendering asserts primary vs secondary line structure.
- Intake child row rendering asserts single-line child indentation/marker/status/title placement.
- Selected Intake Pod target status asserts it is visually described as Intake-related, not generic Ticket or Pod.

Validation reported by coder:
- Passed: `cargo test -p tui workspace_panel --lib`
- Passed: `cargo test -p tui panel_ticket --lib`
- Passed: `cargo test -p tui selected_ticket_intake_child_status --lib`
- Passed: `cargo fmt --check`
- Passed: `git diff --check`
- Ran: `cargo test -p tui multi_pod --lib`
  - Failed in pre-existing/unrelated workspace context tests:
    - `tests::workspace_panel_spawns_orchestrator_with_workspace_context`
    - `tests::workspace_panel_spawns_panel_orchestrator_with_workspace_context`
  - Assertion mismatch concerns expected `/workspace` vs temp test workspace paths, not row rendering/selection changes.

Residual risks:
- No interactive `yoi panel` PTY session was run; visual evidence is covered by focused render/string tests.
- Full `multi_pod` suite remains blocked by unrelated existing failures noted above.

---

<!-- event: review author: yoi-reviewer-00001KV4ZPAD3 at: 2026-06-15T07:08:12Z status: approve -->

## Review: approve

Review result: approve

Evidence checked:
- Reviewed diff `79dda10d..f3b435e7`.
- Changed files limited to:
  - `crates/tui/src/multi_pod.rs`
  - `crates/tui/src/workspace_panel.rs`
- Worktree was clean and on expected branch/commit:
  - `impl/00001KV4ZPAD3-panel-row-hierarchy`
  - `f3b435e7 fix: clarify panel ticket row hierarchy`

Acceptance review:
- Ticket two-line rows now have clearer hierarchy:
  - primary line uses selected marker `▶ ` plus state/title;
  - detail line uses `│ meta ` when selected and `meta` label for secondary metadata.
- Ticket-associated Intake Pod rows now render as child rows with indentation plus `└` / indented `▶` and `Intake Pod: <pod>` labeling.
- Relationship is not color-only; indentation/marker/label communicate hierarchy.
- Selected Ticket vs selected Intake Pod are distinguishable through marker placement, label, color/style, and status text.
- `live`, `restorable`, and `stale` Intake status remains visible.
- Pre-Ticket Intake Pod behavior appears preserved; existing tests still assert pre-ticket sessions are not emitted as `TicketIntakePod` child rows.
- Mouse/keyboard action semantics appear unchanged; changes are limited to row rendering/status text.
- Ticket lifecycle transitions, relation gates, registry persistence, polling/spawn behavior, and Ticket schema were not changed.
- Invalid Ticket placeholder behavior remains non-actionable.
- Alt+Enter handling was not touched.

Focused coverage observed:
- Ticket primary/secondary row rendering contract updated.
- Non-selected alignment/truncation tests updated for `meta`.
- Waiting gate readability remains covered.
- New child-row rendering test covers indentation, single-line behavior, status column, title label, and selected child marker.
- New selected Intake child status test confirms it is not presented as a generic Ticket or generic Pod.
- Existing adjacency/pre-ticket exclusion test remains present.

Validation performed by reviewer:
- Passed: `git diff --check 79dda10d..f3b435e7`
- Passed: `cargo fmt --check`

Validation not run by reviewer:
- Cargo tests were not rerun because review scope was read-only and tests write build artifacts. Coder-reported focused test results were inspected as evidence.

Broader failure assessment:
- The reported broader `cargo test -p tui multi_pod --lib` failure appears unrelated based on diff scope; changes are confined to Panel row rendering/status presentation and tests around that contract, with no workspace-context/runtime setup logic changes.

Conclusion:
- Approved. No blocker remains.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-15T07:09:01Z -->

## Implementation report

Integration report:

Merged implementation branch into orchestration branch:
- branch: `impl/00001KV4ZPAD3-panel-row-hierarchy`
- implementation commit: `f3b435e7 fix: clarify panel ticket row hierarchy`
- merge commit: `8c00a6e9 merge: panel row hierarchy`

Reviewer result:
- `approve` recorded from `yoi-reviewer-00001KV4ZPAD3`.

Validation in Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration`:
- Passed: `cargo test -p tui workspace_panel --lib`
  - 16 passed
- Passed: `cargo test -p tui panel_ticket --lib`
  - 4 passed
- Passed: `cargo test -p tui selected_ticket_intake_child_status --lib`
  - 1 passed
- Passed: `cargo fmt --check`
- Passed: `git diff --check`

Notes:
- Broader `cargo test -p tui multi_pod --lib` remains affected by unrelated workspace-context tests and was not used as a blocker.
- Ticket/Intake visual hierarchy improved without lifecycle/action/schema/polling/spawn semantic changes.
- Orchestrator worktree is clean after validation.

Cleanup planned:
- Stop related coder/reviewer Pods.
- Remove only child implementation worktree/branch for this Ticket.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-15T07:09:01Z from: inprogress to: done reason: merged_validated field: state -->

## State changed

Reviewer approved, implementation branch merged into the orchestration branch, and focused validation passed in the Orchestrator worktree. Marking Ticket done in the orchestration branch.

---

<!-- event: state_changed author: hare at: 2026-06-15T14:59:39Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-15T14:59:39Z status: closed -->

## 完了

Ticket `00001KV4ZPAD3` (`Panel Ticket / Intake Pod row の視覚階層を改善する`) はすでに `state: done` に到達していたため、workspace Panel から close しました。

この Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。


---
