# protocol: ToolResult に summary を分離する

## 背景

`protocol::Event::ToolResult` は現状 `{ id, output: String, is_error: bool }` で、ツール実装側の `llm_worker::tool::ToolOutput` が持っている **`summary: String`** を flatten して捨てている。

各 builtin tool は既に意味のある summary を返している:

- Write (`crates/tools/src/write.rs`): `Created /path (N bytes)` / `Overwrote /path (N bytes)`
- Read / Edit / Glob / Grep も同様に構造化された要約を返している

TUI 側で「俯瞰ビュー」の 1 行サマリや status line の短縮表示に使いたいが、protocol に summary が無いため直接は取れない。`output` の先頭行を切るようなヒューリスティックは脆く、各ツールが意図した要約と一致しない。

本チケットは protocol 層の小さな拡張に閉じる。TUI 側での利用は後続のオーバーホールチケットで行う。

## 要件

- `Event::ToolResult` に `summary: String` を追加する
- `output` と `is_error` は残す（用途が異なる: summary は 1 行、output は詳細本文）
- ツール実装から流れてくる `ToolOutput.summary` がそのまま protocol に乗るように worker / pod 層を通す
- `Event::History` の replay 経路でも summary が欠落しないこと（セッション再接続時に TUI が過去の ToolResult 表示を組み直せるため）

## 完了条件

- `Event::ToolResult` に `summary: String` が存在し、worker 側から流れてくる
- 各 builtin tool (Read / Write / Edit / Glob / Grep) の `ToolOutput.summary` が TUI まで届く
- `Event::History` replay でも summary が取得できる
- 既存 TUI は summary を無視するだけで従前通り動く（UI 側での利用は別チケット）

## 範囲外

- `ToolOutput.content` の protocol 化。現時点では不要。必要になったら別チケット
- summary 文言の規格化や整形。各ツール側の責務
- TUI の summary 利用 (`tickets/tui-fullscreen-overhaul.md` で扱う)
