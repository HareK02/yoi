# TUI: navigation mode / block focus の設計

## 背景

TUI の操作は現在 composer を中心にしており、履歴 block / task 表示 / queued input / system 操作の間を移動する統一的な navigation model はまだない。今後 command mode、manual compact、rollback、Pod picker、queue 編集などが増えると、Ctrl/Alt shortcut だけでは操作体系が散らばる。

一方で、通常入力は最優先で守る必要がある。特に streaming 中の入力取りこぼしや rollback restore、Run 中 input queue を入れたことで、composer の文字入力を暗黙操作で壊さないことが重要になっている。

本チケットは navigation mode のアイデアを保持する設計 ticket であり、すぐ実装する前提ではない。

## アイデア

- 通常は composer mode。
  - 文字入力、Enter submit/queue、`@` / `#` / `/` 補完を優先する。
- `Esc` など明示操作で navigation mode に入る。
  - 履歴 block / task pane / queued input / picker 的 UI に focus を移す。
  - focus があることを視覚的に分かるようにする。
- navigation mode では `j/k` または `↑/↓` で block focus / scroll を行う。
  - `i` / `Enter` / `Esc` で composer に戻る案。
- composer のカーソルが最上行にある時の `↑` で履歴へ抜ける自然操作も候補。
  - ただし multi-line input / IME / completion / typed segment と衝突しやすいため、初期実装では慎重に扱う。
  - 暗黙 focus 移動より、明示 navigation mode を優先する案が安全。
- command mode (`:`) とは分ける。
  - command mode は system command 入力。
  - navigation mode は画面上の対象選択 / scroll / block action。

## 検討事項

- mode 名と status/actionbar 表示。
- composer mode から navigation mode へ入る key。
- navigation mode から composer mode へ戻る key。
- `↑/↓` を composer cursor movement と block focus movement のどちらに使うか。
- block focus の単位。
  - Turn header
  - User message
  - Assistant block
  - Tool call/result
  - System message
  - Task row
  - Queued input row
- focused block に対する action。
  - copy
  - expand/collapse
  - retry/fork/rollback など将来操作
- scrollback と block focus の関係。
- search (`/` ではなく別 key が必要。`/` は WorkflowRef と衝突する可能性)。
- mouse support を入れるか。

## 完了条件（未確定）

- navigation mode の keymap と UI 表示方針が決まる。
- composer 入力を壊さない focus 移動ルールが決まる。
- block focus の最小単位が決まる。
- command mode / queue / rollback / Pod picker と衝突しない。
- 実装 ticket に分割できる。

## 範囲外

- 今すぐの実装。
- command mode の実装（`tickets/tui-command-mode.md`）。
- compact command の実装。
- Vim 完全互換。
