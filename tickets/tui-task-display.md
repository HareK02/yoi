# TUI に Task ストア状態を表示する

## 背景

`tickets/session-todo.md` で導入した `tools::TaskStore` と `TaskCreate / List / Get / Update` ツール群は Pod 側に実装済みで、compaction や resume を跨ぐ永続化も整っている。一方で TUI 側にはまだ表示パスが無く、LLM が Task を作成・更新しても、ユーザーは tool_call ブロックを目で追う以外に状態を把握できない。

`tickets/session-todo-reminder.md` は LLM 側のナッジ（`<system-reminder>` 注入）に専念しており、UI 側からのフィードバックループはスコープ外。Task ツール群を「ユーザーから見える進捗ステータス」として機能させるためには、TUI に常時見える表示が要る。

## 前提

- `TaskStore` / `TaskSnapshot` / `TaskEntry` / `TaskStatus` および `TaskStore::from_history` が `tools` クレートに公開済み
- `TaskStore::from_history` は `TaskCreate` / `TaskUpdate` の tool_call 引数と、compact 時に挿入される `[Session TaskStore snapshot]` system message から決定的に状態を再構成する
- TUI は protocol 経由で tool_call の `name` と最終 `arguments` を受信済み（`ToolCallBlock { name, arguments }`）。system message も `Block::SystemMessage` として観測している
- TUI のレイアウトは `crates/tui/src/ui.rs` で `history / separator / status / input` を縦に積む構成

## 方針

- **TaskStore は TUI 側で再構成する**。Pod から専用 Event を push せず、すでに TUI に届いている tool_call と system message から `TaskStore::from_history` 相当のロジックを回す
  - protocol 表面を増やさない、Pod 側の push 経路も増やさない
  - resume / 再接続時はサーバから来る history replay 経路に乗っかれば自動で復元される
  - 単一情報源（history）に揃うため、Pod と TUI で snapshot がズレない
- TUI 上の表示は 2 ヶ所
  - **入力欄直上のミニビュー（3〜4 行）**: 常時表示。active（`pending` + `inprogress`）優先で最大 2〜3 件を `状態マーク + subject` 1 行ずつ、加えて件数サマリ 1 行
  - **サイドペイン（全件）**: トグルキーで右側に開閉。`pending` / `inprogress` / `completed` / `deleted` を区別して全件列挙、`description` も含める
- `tools` クレートに依存させる（または replay 部分のみを薄いヘルパとして切り出して共有する）。LLM コンテキスト加工原則との整合は自明（history の純粋な再現変換であり context への割り込みではない）

## 要件

### TUI 側の TaskStore 取り回し

- `App` に最新 `TaskStore`（または `TaskSnapshot`）を保持するフィールドを追加する
- 受信パスで以下を観測して TaskStore に反映する
  - `ToolCall` 完了時、`name == "TaskCreate"` / `"TaskUpdate"` なら `arguments` を `TaskStore::from_history` と同じパースで適用
  - `SystemMessage` 観測時、本文が `[Session TaskStore snapshot]` で始まるなら `parse_compact_snapshot_text` 相当で snapshot に置き換え
- 初回 history replay の際も同じ経路を通せば、TUI 起動時点の状態が自動的に復元される
- 部分的引数（streaming 中）は適用しない。tool_call が完了して `arguments` が確定したタイミングだけ反映する

### TUI ミニビュー

- 入力欄の直上に固定で 3〜4 行の領域を確保
- active（`pending` + `inprogress`）を優先して最大 2〜3 件、`状態マーク + subject` を 1 行ずつ表示。`subject` が改行を含む場合は先頭行のみ
- 件数サマリ 1 行（`pending` / `inprogress` / `completed` / `deleted` の内訳）
- active 0 件のときの見せ方は実装時に決める（サマリ行のみ残すか、領域ごと畳むか）

### TUI サイドペイン

- トグルキーで右側に開閉する横分割レイアウト（割当キーは実装時に決める）
- 開いている間は全件を表示。各エントリは `taskid` / `status` / `subject` / `description` を含める
- スクロール対応（タスク数が画面高を超えてもよい）

### レイアウトと一貫性

- ミニビューとサイドペインは同じ最新 TaskStore を見る（表示間でズレないこと）
- 既存の history / status / input の幅・高さ計算が壊れないこと
- completion popup（`@` / `#` / `/`）と表示位置が干渉しないこと

## 完了条件

- LLM が `TaskCreate` / `TaskUpdate` を呼ぶと、TUI が tool_call 完了タイミングで状態を反映し、ミニビューに即時反映される
- トグルキーでサイドペインが開閉し、`completed` / `deleted` を含む全件が確認できる
- resume / 再接続時、history replay 経路から TaskStore が自動復元され、ミニビューとサイドペインに反映される
- compact 後の history（末尾に snapshot system message）でも、それを起点に正しく再構成される
- ミニビュー / サイドペインの導入で既存レイアウト（history / status / input、completion popup）が崩れない

## 範囲外

- protocol への新規 Event 追加。本チケットでは行わない（必要が出たら別途検討）
- TUI からの Task 編集（追加・更新・削除）。表示専用、編集経路は LLM ツール経由のみ
- 表示密度モード（Detail / Normal / Overview）への連動。ミニビューは常時、サイドペインはトグル制
- LLM 側のナッジ（`tickets/session-todo-reminder.md` で別途実装）
- サイドエージェント / sidechain の TaskStore 表示（main Pod の TaskStore のみ対象）

## 参照

- 実装: `crates/tools/src/task.rs`（`TaskStore::from_history` / `parse_compact_snapshot_text` / `TaskStatus` / `TaskEntry`）、`crates/tui/src/ui.rs`、`crates/tui/src/block.rs`（`ToolCallBlock` / `SystemMessage`）
- 設計指針: `AGENTS.md`「LLM コンテキストの加工原則」（history からの決定的再構成は許容変換）
- 関連チケット: `tickets/session-todo.md`（Task ツール本体）、`tickets/session-todo-reminder.md`（LLM 側ナッジ）
