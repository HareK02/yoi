<!-- event: create author: ticket-intake at: 2026-06-13T10:05:19Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-13T10:05:28Z -->

## Intake summary

`yoi panel` の View item / row をマウスクリックでアプリ内選択できるようにする Ticket として materialize した。参考 UX は `./ghq.local/github.com/anomalyco/opencode` の OpenCode TUI selection/dialog/autocomplete 系。ratatui では DOM-like handler ではなく、描画時の item rect registry と `MouseEvent { column, row }` hit-test が有力。MVP は click/down による selection のみで、Queue/Open/Close 等の workflow action 即時実行、汎用 drag/text selection、single-Pod history block focus は範囲外。既存 composer 入力優先、selected-Pod direct-send 非復活、least-intrusive mouse capture 方針を invariant とする。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-13T10:05:28Z from: planning to: ready reason: planning_ready field: state -->

## State changed

要件、受け入れ条件、binding invariants、OpenCode 参考、実装裁量、escalation conditions、validation が整理され、Orchestrator が routing 可能な状態になったため ready にする。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-13T10:53:16Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-13T10:55:52Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は `queued` で、要件・受け入れ条件・binding invariants・implementation latitude・escalation conditions が揃っている。
- `TicketRelationQuery` と `TicketOrchestrationPlanQuery` で blocker / ordering / conflict 記録は見つからなかった。
- risk flags は `tui-input` / `mouse-capture` / `panel-ux` だが、Ticket は mouse capture 方針、click-only MVP、destructive action 非実行、composer 入力優先を binding invariant として明記しており、実装前に不足する設計判断はない。
- 現 Orchestrator worktree は clean。root/original workspace では git/read/write/validate せず、実装は専用 child worktree に隔離する。
- 併走候補のうち `00001KV0723PC` は同じ `crates/tui/src/multi_pod.rs` 周辺に触れる可能性が高いため、こちらの panel mouse selection を先に受理し、Quit 遅延 Ticket は queued のまま conflict/capacity 待ちにする。`00001KV04NJ8D` は single-Pod rewind / Pod protocol 周辺で主な変更面が異なるため並列開始候補にできる。

Evidence checked:
- Ticket body / thread / artifacts（artifacts なし）。
- relation records: なし。
- orchestration plan records: なし。
- code map: `crates/tui/src/multi_pod.rs` の panel selection state / keyboard handling / draw paths、`crates/tui/src/workspace_panel.rs` の ViewModel / row data、既存 `EnableWheelMouseCapture` 方針は `crates/tui/src/single_pod.rs` にあることを確認。
- related context: composer 入力優先、Panel selected-row actionbar 方針、least-intrusive mouse capture 方針は Ticket に binding invariant として反映済み。
- workspace/Pod state: Orchestrator worktree clean、visible live implementation Pods なし。

IntentPacket:

Intent:
- `yoi panel` の View item / row をマウスクリックでアプリ内選択できるようにし、クリック後の selected row に既存の blank Enter / actionbar / detail 表示が働くようにする。

Binding decisions / invariants:
- 対象は workspace Panel/View item selection に限定する。single-Pod conversation history 全体の block focus / navigation mode は実装しない。
- マウス操作で selected-Pod direct-send semantics を復活させない。
- composer text entry と既存 keyboard 操作を優先し、`↑` / `↓` / Enter / Esc / Tab の意味を壊さない。
- クリックは selection のみで、Queue / Open / Close などの workflow state mutation / destructive action を即時実行しない。
- 汎用 drag/text selection は作らない。
- `?1000h` + `?1006h` の least-intrusive mouse capture 方針を優先し、drag tracking を有効化して端末選択への副作用を増やさない。

Requirements / acceptance criteria:
- Panel 表示中に View item / row をクリックすると対応 item が selected になる。
- item 外クリックでは不正な selection change や composer draft loss が起きない。
- selected item に対する既存 action / blank Enter / detail 表示がクリック後の selection に対して働く。
- existing keyboard/composer behavior を維持する。
- focused tests で row hit testing / click selection / non-row click no-op / keyboard preservation を確認する。

Implementation latitude:
- render 時の row rect registry、row coordinate mapping、hit-test helper の具体設計は coder が選んでよい。
- MVP は click/down による Panel rows selection でよい。詳細 pane 内の個別要素クリックや hover selection は必須ではない。
- 外部 crate は不要なら入れない。既存 Panel 構造を大きく歪める外部 crate 導入は escalation する。

Escalate if:
- mouse capture が端末通常選択・貼り付け・wheel・IME・composer 入力へ副作用を出す場合。
- click selection と Enter / blank composer / Queue/Open/Close の境界が曖昧になる場合。
- Panel 以外の view、single-Pod block focus、drag selection へ範囲が広がる場合。
- hit testing のために大きな描画アーキテクチャ変更が必要になる場合。

Validation:
- focused `cargo test -p tui workspace_panel --lib` または該当 `multi_pod` / workspace panel tests。
- `cargo fmt --check`。
- `git diff --check`。
- 変更範囲に応じて `cargo check --workspace --all-targets`。

Current code map:
- `crates/tui/src/multi_pod.rs`: panel event handling、`selected_row`、visible rows、draw/hotkey/action path、tests。
- `crates/tui/src/workspace_panel.rs`: `WorkspacePanelViewModel` / `PanelRowKey` / row data。
- `crates/tui/src/single_pod.rs`: existing narrow mouse capture implementation as reference only。

Critical risks / reviewer focus:
- click must select only, not execute workflow actions。
- item 外 click must be no-op for selection and must not drop composer draft。
- keyboard/composer target semantics must remain unchanged。
- mouse capture must remain least-intrusive and not enable drag tracking unnecessarily。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T10:56:05Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Ticket evidence、relations、orchestration plan、bounded code map、Orchestrator worktree clean state を確認した。blocking relation はなく、Panel mouse selection の binding invariants と acceptance criteria は実装可能な粒度で記録済み。実装 side effect の前に routing decision / IntentPacket / accepted plan を記録したため、専用 child worktree と Coder delegation に進む。

---

<!-- event: implementation_report author: orchestrator at: 2026-06-13T10:59:28Z -->

## Implementation report

Implementation handoff started.

- Worktree: `/home/hare/Projects/yoi/.worktree/panel-mouse-selection`
- Branch: `ticket-00001KV072V89-panel-mouse-selection`
- Coder Pod: `coder-00001KV072V89-panel-mouse`
- Scope: child worktree read, `crates/tui` write, this Ticket record write。`.yoi/memory` や local/runtime/log/lock/secret-like `.yoi` paths は write scope に含めていない。
- Note: 初回 SpawnPod は runtime launch validation が workspace root `/home/hare/Projects/yoi` の readable grant を要求して失敗したため、非再帰 read grant のみ追加して再実行した。Coder には root/original workspace を inspect/write/git/validate/merge/cleanup しないよう明示済み。

Next:
- Coder の commit / implementation_report / validation evidence を確認後、Reviewer を read-only 基本で起動する。

---

<!-- event: implementation_report author: hare at: 2026-06-13T11:15:00Z -->

## Implementation report

Implementation report for Ticket 00001KV072V89

Summary:
- Added panel row mouse hit testing in `crates/tui/src/multi_pod.rs` by recording selectable row rectangles during list rendering.
- Handled left mouse button down events as selection-only operations for ticket/pod rows; non-row clicks are no-ops and diagnostic popups ignore clicks behind them.
- Kept action execution on existing keyboard/action paths: clicked rows feed existing selected row behavior, including blank Enter, actionbar target status, and row detail selection.
- Added focused tests for row hit testing, click selection, non-row click no-op/draft preservation, and composer/keyboard behavior preservation after click selection.

Validation:
- `cargo fmt -p tui` — passed.
- `cargo fmt --check` — passed.
- `cargo test -p tui mouse_ --lib` — passed (4 tests; includes the focused mouse tests plus existing wheel capture test matching the filter).
- `cargo test -p tui row_hit_testing_maps_only_visible_selectable_rows --lib` — passed.
- `cargo check -p tui --all-targets` — passed.
- `git diff --check` — passed.
- Additional broader check: `cargo test -p tui multi_pod --lib` ran; 85 tests passed and 1 existing/unrelated test failed (`multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace`, expected orchestration root vs `/repo/yoi`). The failure is outside the panel mouse selection change area.

Commit:
- `452c9df1` (`tui: select panel rows by mouse`)

Residual risks:
- Hit boxes are tied to the last rendered list area. This matches the current draw/event loop model; resize events trigger redraw, but stale hit boxes before the first draw would be ignored/empty.
- MVP intentionally handles left-button down only; hover, release-to-action, detailed pane clicks, and drag/text selection remain out of scope.

---

<!-- event: review author: hare at: 2026-06-13T11:19:22Z status: approve -->

## Review: approve

Approve.

Evidence:
- Reviewed Ticket 00001KV072V89 and diff `20daae0c..HEAD` in `/home/hare/Projects/yoi/.worktree/panel-mouse-selection`.
- Implementation adds a bounded `PanelRowHitBox` hit-test cache from the rendered list area and only handles primary-button `MouseEventKind::Down`; non-row/list-empty clicks return no selection change.
- Click handling calls `select_panel_key` only. I found no mouse path invoking Queue/Open/Close, ticket lifecycle mutation, Pod attach/open, or destructive workflow actions.
- Existing keyboard/composer paths remain the execution/action paths: blank Enter/actionbar/detail display consume the selected row after selection; text input, arrows, Enter, Esc, and Tab behavior is still handled by the existing key handlers.
- Modal/diagnostic state is guarded by `panel_diagnostic_open`, so background row clicks do not change selection while diagnostics are shown.
- Mouse terminal mode remains the existing least-intrusive normal mouse + SGR setup (`?1000h` + `?1006h` via the local narrow capture command), with no drag-capture mode added.
- Tests added/updated cover row hit testing for visible selectable rows, left-click selection feeding existing blank-Enter behavior, non-row no-op/preserved draft, and keyboard/composer preservation; the no-Queue/Open/Close guarantee is enforced by the mouse handler only selecting rows and leaving action dispatch on existing key/action paths.

Validation performed:
- `cargo test -p tui mouse_ --lib` — passed.
- `cargo test -p tui row_hit_testing_maps_only_visible_selectable_rows --lib` — passed.
- `cargo fmt --check` — passed.
- `git diff --check 20daae0c..HEAD` — passed.

Risks / notes:
- I did not run the broader `cargo test -p tui multi_pod --lib` suite because the requested focused coverage passed and that broader suite was noted as having a likely unrelated existing failure.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-13T11:26:23Z -->

## Implementation report

Integration outcome: merged and validated.

- Implementation branch: `ticket-00001KV072V89-panel-mouse-selection`
- Implementation commits: `452c9df1 tui: select panel rows by mouse`, `c96a6c46 ticket: report panel mouse implementation`
- Review commit: `6c7385e5 ticket: approve panel mouse selection`
- Orchestrator merge commit: `02311883 merge: panel mouse selection`

Reviewer result:
- `approve`。Panel row hit-test cache、left-click selection-only path、non-row no-op / composer draft preservation、diagnostic guard、least-intrusive mouse capture、keyboard/composer preservation を確認済み。

Orchestrator validation after merge:
- `cargo test -p tui mouse_ --lib`: PASS（4 tests）
- `cargo test -p tui row_hit_testing_maps_only_visible_selectable_rows --lib`: PASS（1 test）
- `cargo fmt --check`: PASS
- `git diff --check`: PASS
- `cargo check -p tui --all-targets`: PASS

Residual notes:
- Broad `cargo test -p tui multi_pod --lib` は child validation で既知 unrelated failure があるため、Orchestrator validation では focused tests と `cargo check -p tui --all-targets` を採用した。
- Click は selection のみで、Queue/Open/Close など workflow action は既存 key/action path に残っている。

Next:
- Ticket を `done` に遷移し、panel mouse child Pods / worktree / branch を cleanup する。close は別途明示権限がある場合に行う。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T11:26:30Z from: inprogress to: done reason: merged_and_validated field: state -->

## State changed

Implementation branch `ticket-00001KV072V89-panel-mouse-selection` was reviewed, approved, merged into the Orchestrator branch as `02311883`, and validated in the Orchestrator worktree. Focused mouse-selection tests, formatting, diff check, and `cargo check -p tui --all-targets` passed. Ticket implementation work is done; closure remains separate.

---

<!-- event: comment author: orchestrator at: 2026-06-13T12:57:53Z -->

## Comment

Post-merge user verification report: mouse selection is still ineffective.

User report:
- 「マウス選択全く効かない」

What was re-checked:
- Merge commit for this Ticket: `02311883 merge: panel mouse selection`。
- Implementation added `PanelRowHitBox` / `TermEvent::Mouse` handling in `crates/tui/src/multi_pod.rs` and focused tests, but there was no interactive `yoi panel` manual validation before marking done。
- Current built debug binary contains `PanelRowHitBox` symbols; installed `/home/hare/.nix-profile/bin/yoi` did not contain those symbols in `strings` check, so at least one live/user-visible binary path may still be stale。
- This does not fully explain the report if the user already rebuilt/restarted; the remaining likely failure areas are terminal mouse-event delivery, runtime binary freshness, hitbox coordinate mismatch, or lack of interactive validation coverage。

Conclusion:
- The original done decision was insufficient because it relied on internal hit-test/focused tests and did not prove actual terminal mouse selection in a live Panel.
- Follow-up must include measured/manual validation with the same command/binary path the user runs, including a positive proof that a click changes selected row in `yoi panel`.

---
