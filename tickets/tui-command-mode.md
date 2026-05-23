# TUI: command mode / command registry / completion

## 背景

TUI には通常の user message 入力に加えて、Compact、rollback、queue 操作、Pod attach/restore など、LLM に送らない制御操作が増えつつある。既存の入力欄には `@` FileRef、`#` KnowledgeRef、`/` WorkflowRef があり、system command を `/compact` 等に寄せると既存の参照・補完体系と衝突する。

また、制御操作を Ctrl/Alt shortcut だけで増やしていくと発見性が低く、keybinding が破綻しやすい。通常入力を守りつつ、system 操作の正規導線として `:` command mode を用意する。

本チケットは command mode の土台のみを実装する。Compact 実行など具体的な system command は後続 ticket で扱う。

## 方針

- `:` で command mode に入る。
- command mode 中は input area を command input として扱い、通常 user message と明確に分離する。
- `Esc` で command mode を抜け、通常 composer に戻る。
- `Enter` で command registry に dispatch する。
- 未知 command / 引数不正は Pod に送らず TUI 側で診断する。
- command mode の command は LLM への user message として history に混入しない。
- 既存の `@` / `#` / `/` 補完や typed segment は通常 composer 側の機能として維持する。

## 要件

- TUI に mode state を追加する。
  - 少なくとも `Composer` と `Command` を区別する。
  - status / actionbar 等で現在 command mode であることが分かる。
- `:` key で command mode に入る。
  - 通常 composer に `:` を入力したい場合の escape / literal 入力方針は実装時に決める。
  - 最小実装では、空 composer または通常 mode で `:` を押した時に command mode へ入る等、誤爆しにくい条件にしてよい。
- command mode の入力編集を実装する。
  - text insert / backspace / cursor movement / clear / Esc / Enter。
  - 通常 composer の buffer を破壊しない。
- command registry を追加する。
  - command name、aliases、usage、description、argument parser、実行可否判定、executor を登録できる構造にする。
  - 最初は built-in の no-op / help / unknown 診断だけでもよい。
  - 後続 `tui-system-command-compact` が `compact` command を追加できる形にする。
- completion / suggestion を実装する。
  - prefix に応じた command 候補を actionbar / popup / input 下部などに表示する。
  - Tab で候補補完するか、候補表示だけにするかは実装時に決める。既存 completion との衝突を避ける。
- tests を追加する。
  - `:` で command mode に入り、Esc で戻る。
  - command 入力が通常 composer を壊さない。
  - unknown command が user message として送信されない。
  - registry から候補が出る。
  - command mode 中の Enter が `Method::Run` を送らない。

## 完了条件

- TUI に command mode があり、通常 composer と視覚的・状態的に区別できる。
- command registry があり、後続 ticket で `compact` などを追加できる。
- command completion / suggestion が最低限機能する。
- command mode の入力は LLM user message として送信されない。
- 既存の composer 入力、`@` / `#` / `/` 補完、Run 中 queue、rollback restore を壊さない。
- `cargo fmt --check`
- `cargo check --workspace`
- `cargo test -p tui`

## 範囲外

- `compact` command の実行。
- manual rollback command の実行。
- Pod attach/restore command の実行。
- navigation mode / block focus。
- Vim 互換の本格 command line。
