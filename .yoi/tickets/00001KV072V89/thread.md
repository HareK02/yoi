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
