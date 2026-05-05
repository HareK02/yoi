# TUI: Assistant 応答の Markdown スタイル表示

## 背景

LLM の出力は実質 Markdown だが、TUI は `Block::AssistantText { text }` を
`push_padded_lines` で 1 行ずつ素のテキストとして
`Style(MessageKind::Assistant)` に流しているだけで、`**強調**` /
`` `code` `` / `# 見出し` / `- list` 等の記号がそのまま見える状態になっている
（`crates/tui/src/ui.rs:592-595, 640-648`）。スタイルが付かないため、
構造のあるアシスタント応答は読みにくい。

ratatui 0.30 の `Vec<Line<'static>>` で表現できる範囲のスタイル付けで
十分目的を満たせる。既存の `wrap_line_into`（`crates/tui/src/ui.rs:473-`）が
span 単位のラップを既に実装しているため、Markdown レンダラは
スタイル付きの `Vec<Line>` を返すだけでよく、ラップ／スクロール／overview
畳み込みの仕組みを変える必要はない。

## 方針

- `pulldown-cmark` を `tui` クレートの依存に追加し、Event ストリームを
  既存の `MessageKind` / `ratatui::style::Style` 体系へ畳み込む小さな
  自前レンダラを `crates/tui/src/markdown.rs` に置く。
- レンダラの公開面は `render(text: &str, base: Style) -> Vec<Line<'static>>`
  程度の 1 関数。`Block::AssistantText` の `Mode::Detail` / `Mode::Normal`
  描画から呼ぶ。`Mode::Overview` は現行通り 1 行畳み込み（Markdown 記号
  含めて表示しても情報量はほぼ同じなので素のテキストでよい）。
- ストリーミング中の不完全要素（未閉鎖の `**` や開きっぱなしのフェンス）
  は CommonMark の流儀（テキスト扱い／EOF で閉じる）に任せる。挙動が
  破綻する場合だけ末尾要素を素のテキストにフォールバックする小さな
  後処理を入れる余地を残す。
- `tui-markdown` クレートは採用しない。syntect 依存でビルドが肥大する
  割にカスタマイズが効かず、本クレートの色味（`MessageKind`
  パレット）との整合を握りにくいため。

## 対応する Markdown 要素

最小限の "対応できる範囲" を以下に限定する。CommonMark + GFM の一部。

- 強調: `**bold**` / `*italic*` / `~~strike~~`（GFM）
- インラインコード: `` `code` ``
- フェンスコードブロック: ` ```lang ` / ` ``` `（言語タグは無視、
  ブロック全体を等幅・低彩度の背景／前景で塗る）
- 見出し: H1〜H4（H5/H6 は H4 と同等）
- 箇条書きリスト: `-` / `*` / `+`、ネスト可（深さ分インデント）
- 順序リスト: `1.` / `1)`、ネスト可（番号は元の値で表示）
- 引用: `> ...`（ネスト可）
- 水平線: `---` / `***`
- リンク: `[text](url)` の `text` をリンク色で着色（URL は表示しない）

## 範囲外

- 表（GFM table）
- 画像 `![alt](src)`（テキストとしても表示しない）
- HTML パススルー（タグはそのまま生テキストで出る）
- 数式（`$...$` / `$$...$$`）
- コードブロックの syntax highlighting
- リンクのターミナルクリック（OSC 8）／URL の自動表示
- `Thinking` 本文 / `SystemMessage` への適用
  （同じ `markdown::render` を後で差せばよい。本チケットは
  `Block::AssistantText` のみ）
- ライブストリーム最中の "途中要素のフォールバック" の作り込み
  （CommonMark のデフォルト挙動で破綻が見えたら別チケット）

## 完了条件

- アシスタント応答に含まれる上記要素が、それぞれ視認可能な
  スタイルで描画される。
- ストリーミング中、テキストが追記されるたびに描画が更新され、
  フェンスコードブロックの開きが先に着いて中身が後から流れる
  ようなケースでも、テキスト全体の見た目が大きく崩れない。
- `Mode::Detail` / `Mode::Normal` で Markdown スタイルが、
  `Mode::Overview` では従来通りの 1 行畳み込みが出る。
- 既存の `wrap_line_into` によるラップ・右パディング・スクロール
  が引き続き機能する（行幅計算が乱れない）。

## 影響範囲

- `crates/tui/Cargo.toml`: `pulldown-cmark` を追加（`cargo add` 経由）。
- `crates/tui/src/markdown.rs`: 新設。`render(&str, Style) -> Vec<Line<'static>>`。
- `crates/tui/src/ui.rs`: `Block::AssistantText` 分岐で Markdown
  レンダラを呼ぶ。`Mode::Overview` は現行のまま。
- `crates/tui/src/main.rs` または `lib.rs`: 新モジュールの宣言。

## Review

- 状態: Approve
- レビュー詳細: [./tui-assistant-markdown.review.md](./tui-assistant-markdown.review.md)
- 日付: 2026-05-05
