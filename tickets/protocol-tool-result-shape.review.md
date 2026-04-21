# Review: protocol: ToolResult に summary を分離する

## 前提・要件の確認

- **`Event::ToolResult` に `summary: String` を追加**: 満たしている（`crates/protocol/src/lib.rs:112-125`）。
- **`output` と `is_error` は残す（用途: summary は 1 行、output は詳細本文）**: **満たしていない**。実装は `output: String` を `content: Option<String>` にリネーム + オプショナル化している（`crates/protocol/src/lib.rs:117-124`）。詳細本文フィールド自体は残っているが、名前と型が要件と食い違う。後述 Blocking 参照。
- **`ToolOutput.summary` がそのまま protocol に乗るように worker / pod 層を通す**: 満たしている。worker に `on_tool_result` コールバックを追加（`crates/llm-worker/src/worker.rs:302-323, 777-780`）、pod コントローラーで `Event::ToolResult` を発火（`crates/pod/src/controller.rs:189-197`）。従来この variant を produce するコードは存在しなかったため、この経路新設は必須。
- **`Event::History` replay でも summary が欠落しないこと**: 満たしている。`Item::ToolResult` の serde 形は既に `{type: "tool_result", summary, content}` で（`crates/llm-worker/src/llm_client/types.rs:71-82`）、TUI の `restore_history` がそれに合わせて `item["summary"]` を読むように修正された（`crates/tui/src/app.rs:342-348`）。
- **既存 TUI は summary を無視するだけで従前通り動く**: 満たしている。live handler / restore_history ともにコンパイル通過、意味的にも summary を表示するようになっている（`crates/tui/src/app.rs:140-155, 342-348`）。

## アーキテクチャ・スコープ

- worker 既存の callback 流儀（`on_tool_use_block` / `on_turn_end` / `on_usage` / `on_warning`）と整合した形で `on_tool_result` を追加しており、拡張方法として proportionate。
- callback が `post_tool_call` interceptor と `tool_output_limits` truncation の後に fire されるという不変条件が明示され（`worker.rs:310-316, 742-780`）、履歴に入る payload と一致する。この設計選択は妥当で、documentation も揃っている。
- `Mutable` / `Locked` いずれの遷移経路でも `tool_result_cbs` が運搬されており、state 型安全性を崩していない（`worker.rs:1300, 1375`）。
- TUI 側修正は protocol 変更に追従する最小限で、`tui-fullscreen-overhaul.md` の範疇（ブロックモデル、折りたたみ、レンダラ dispatch 等）に踏み込んでいない。スコープは守られている。
- `restore_history` の `item["output"]` → `item["summary"]` は一見 drive-by だが、`Item::ToolResult` の serde 形は元から `{summary, content}` であり、変更前は History 経路で **常に空文字** を表示していた既存バグ。「`Event::History` replay でも summary が欠落しない」「既存 TUI は summary を無視するだけで従前通り動く」という要件を満たすために必要な修正で、スコープ内と判断できる。

## 指摘事項

### Blocking

- **`output` → `content: Option<String>` の改名と Option 化は要件違反**（`crates/protocol/src/lib.rs:117-124`, `crates/pod/src/controller.rs:191-196`, `crates/tui/src/app.rs:140-154`）。
  - 要件は「`output` と `is_error` は残す」。実装は `output: String` を `content: Option<String>` に差し替えている。
  - かつチケットの「範囲外」セクションが明示的に "`ToolOutput.content` の protocol 化。現時点では不要。必要になったら別チケット" と記している。`ToolOutput` 側のフィールド名 (`content`) と意味（prune で消せる詳細本文） を protocol に持ち込んでいるため、これは「範囲外」に真正面から抵触する。
  - ツール内部モデルと名前を揃えたい動機は理解できるが、意思決定は別チケットで扱うべきで、この PR で先取りすべきではない。
  - **対処方針**: 以下のいずれか。
    1. チケットの指示通り `output: String` に戻す（意味的に truncation 後の詳細本文を入れる）。pod 側は `result.content.unwrap_or_default()` を詰めるか、`summary` をフォールバックにするかを worker 層の semantics に合わせて選ぶ。
    2. 仕様変更として妥当だと判断するなら、チケット本文の要件行と「範囲外」の該当項目を **先に** 書き換えてから実装を通す（CLAUDE.md "b. 詳細化や前提の変化" の手順）。

### Non-blocking / Follow-up

- `Event::ToolResult` の wire 互換性に破壊変更が入る点はチケット範囲内なのでそれ自体は問題ないが、Blocking を (1) で解消する場合と (2) で解消する場合で最終形が変わる。方針確定後にテストの assertion (`lib.rs:520-542`) と `omits_absent_content` テストの取り扱いを合わせて決めること。
- `tool_result_cbs` の error-path カバレッジはない。`ToolResult::error(...)` が `summary: error_message, content: None, is_error: true` で emit されることを示すテストを callback_test に追記するのは follow-up として価値がある（既存テストは success ケースのみ）。

### Nits

- なし。コード本体の命名・module 配置・docstring はプロジェクト慣習と整合。

## 判断

**Request changes** — 要件の文面と「範囲外」に明示的に反する protocol フィールド名変更（`output` → `content: Option<String>`）が含まれており、これは判断を伴う仕様変更なのでチケット更新を先行させるか、チケット通りに戻す必要がある。それ以外の worker / pod / TUI / テストの取り扱いはすべて proportionate で問題なし。
