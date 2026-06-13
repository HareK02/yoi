---
title: 'Workspace panel の View item をマウスで選択できるようにする'
state: 'queued'
created_at: '2026-06-13T10:05:19Z'
updated_at: '2026-06-13T10:53:16Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui-input', 'mouse-capture', 'panel-ux']
queued_by: 'workspace-panel'
queued_at: '2026-06-13T10:53:16Z'
---

## Background

`yoi panel` で、マウスによる選択を端末側のテキスト選択に依存する現行スタイルではなく、Panel/View の item に限ってアプリ内でネイティブに選択できるようにしたい。

現状の TUI は端末テキスト選択を温存するため、mouse capture を限定している箇所がある。Yoi には既に `EnableWheelMouseCapture` があり、`?1000h` + `?1006h` により wheel と button press/release を扱える一方、drag-motion tracking は要求しない方針になっている。この方針は、click-to-select では維持できる可能性が高い。

参考 UX として、`./ghq.local/github.com/anomalyco/opencode` にある OpenCode TUI の select/dialog/autocomplete 系の挙動を参照する。OpenCode は TSX component tree 側で `onMouseOver` / `onMouseDown` / `onMouseUp` を使える構造だが、Yoi は ratatui なので DOM-like handler の移植ではなく、描画済み item rectangle を `MouseEvent { column, row }` で hit-test する設計になる。

## Requirements

- `yoi panel` の View item / row に対するマウスクリックで、対応する item が selected row になる。
- 選択可能範囲は Panel/View の item 領域に限定する。
- item 外クリックは composer 入力や既存状態を不必要に壊さない。
- composer の通常テキスト入力・既存キーボード操作は維持する。
- クリック選択と既存の `↑` / `↓` / Enter / Esc / Tab 等の意味が矛盾しない。
- mouse wheel scrolling など既存のマウス挙動がある場合は、不要に退行させない。
- UI help / actionbar / diagnostics に、必要なら mouse selection が可能であることを短く反映する。

## Acceptance criteria

- Panel の表示中に View item / row をクリックすると、その item が selected になる。
- selected item に対する既存の blank Enter / actionbar action / detail 表示が、クリック後の選択に対して働く。
- item 外クリックでは不正な selection change や composer draft loss が起きない。
- composer 文字入力、矢印キー選択、Tab target switching、Esc の既存挙動が保たれる。
- 端末 text selection を完全に代替する汎用ドラッグ選択はこの Ticket では実装しない。
- Focused TUI tests が、row hit testing / click selection / non-row click no-op / existing keyboard behavior preservation をカバーする。
- 妥当な検証として少なくとも focused `cargo test -p tui ...`、`cargo fmt --check`、`git diff --check` を実施する。

## Binding decisions / invariants

- 対象は `yoi panel` の View item selection。single-Pod conversation history 全体の block focus / navigation mode はこの Ticket の主目的にしない。
- マウス操作で selected-Pod direct-send semantics を復活させない。
- composer text entry を優先する既存方針を壊さない。
- クリックは selection のための操作であり、Queue / Open / Close などの destructive or workflow state mutation action を即時実行しない。実行は既存の明示 action path に委ねる。
- terminal の任意範囲テキスト選択を再現する汎用 selection 機能は作らない。
- 既存の least-intrusive な `EnableWheelMouseCapture` 方針を優先し、drag tracking を有効化して端末選択への副作用を増やす変更は避ける。

## Implementation latitude

- 具体的な hitbox 管理、row coordinate mapping、描画時の layout rect 保存方法は実装者が調査して選べる。
- ratatui では OpenCode の DOM-like mouse handlers は使えないため、render 時に item rect registry を作り、`MouseEvent { column, row }` を hit-test する実装が有力。
- 必要なら最初は Panel rows のみ対象にし、詳細 pane 内の個別要素クリックは範囲外にしてよい。
- OpenCode 風に hover で active selection を動かすかどうかは、この Ticket の必須条件ではない。MVP は click/down による selection だけでよい。
- click release で item action を実行する挙動は MVP では必須にしない。安全側では selection のみに留める。
- UI 表示文言は短く保ち、status bar を冗長化しない。
- 外部 crate を使う場合は、`tui-widget-list` の hit-test、`ratatui-interact` の click region/focus 管理などを候補として検討できる。ただし既存 Panel 構造を大きく置き換える必要がある場合は自前の小さい hit-test registry を優先する。

## Readiness

- readiness: implementation_ready
- risk_flags: [tui-input, mouse-capture, panel-ux]

## Escalation conditions

- Mouse capture を有効化すると端末の通常選択・貼り付け・wheel・IME・composer 入力に副作用が出る場合。
- Click selection と Enter / blank composer open / Ticket Queue などの workflow action の境界が曖昧になる場合。
- Panel 以外の TUI view、single-Pod history block focus、drag selection まで自然に巻き込みたくなる場合。
- ratatui/crossterm の制約で item hit testing のために大きな描画アーキテクチャ変更が必要になる場合。
- 外部 crate 導入が Panel の ViewModel / custom rendering を大きく歪める場合。

## Validation

- Focused tests around workspace panel / multi_pod mouse event handling.
- Existing workspace panel keyboard/composer tests.
- `cargo test -p tui workspace_panel --lib` または該当 focused tests。
- `cargo fmt --check`
- `git diff --check`
- 必要に応じて `cargo check --workspace --all-targets`

## Related work

- `00001KSKBPPMR` — TUI navigation mode / block focus design。広い設計背景。
- `00001KTJ0B3G0` — bare letter shortcuts removal。composer 入力優先の既存決定。
- `00001KTFEVH3R` — Panel row/action simplification。selected-row actionbar 方針。
- `./ghq.local/github.com/anomalyco/opencode` — OpenCode TUI の select/dialog/autocomplete 系 mouse UX 参考。
