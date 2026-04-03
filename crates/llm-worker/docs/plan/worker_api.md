# Worker API/DSL 実装計画

## 目的

- [Open Responses](https://www.openresponses.org)(以後"OR")に準拠した正規化を前提に、
  Item/Part の2段スコープを扱える Worker API を設計する。
- APIの煩雑化を防ぐため、worker.on_xxx として公開するのを避けつつ、
  Text/Thinking/Tool など型の違いを静的に扱える DSL を提供する。

## 方針

- 内部は Timeline が Event を正規化し、Item/Part/Meta
  を単一ストリームとして扱う。
- API では Item/Part 型ごとに ctx を持てるようにし、DSL
  で記述の冗長さを削減する。
- まず macro_rules! 版を作り、必要なら proc-macro に拡張する。
- Item/Part の型パラメータはクレートが公開する Kind 型を使う。

## 仕様の前提

- Item は OR の item (message, function_call, reasoning など) に対応する。
- Part は OR の content part (output_text, reasoning_text など) に対応する。
- Item は必ず start/stop を持つ。Part は Item 内で複数発生し得る。
- Item/Part の型指定は `Item<Message>` / `Part<ReasoningText>` のように書く。

---

## 設計ステップ

### 1. 内部イベントモデルの整理 ✅

- Event を Item/Part/Meta の3層に整理する。

#### 実装状況: ✅ 完了

`timeline/event.rs` に3層のイベントモデルを実装済み:
- **Meta イベント**: `Ping`, `Usage`, `Status`, `Error`
- **Block イベント**: `BlockStart`, `BlockDelta`, `BlockStop`, `BlockAbort`
- Block 種別として `Text`, `Thinking`, `ToolUse`, `ToolResult` を定義
- `DeltaContent` で `Text(String)`, `Thinking(String)`, `InputJson(String)` を区別

#### 計画との差分

- 計画の「ItemEvent / PartEvent は型パラメータで区別する」方針ではなく、`BlockType` enum + `DeltaContent` enum で区別する設計を採用。Item/Part の2段ではなく Block/Delta の2段として実装。
- OR の Item 型は `llm_client/types.rs` の `Item` enum で直接モデル化済み（`Message`, `FunctionCall`, `FunctionCallOutput`, `Reasoning`）。

### 2. スコープの二段化 ✅

- Item ctx: Item 型ごとに1つ
- Part ctx: Part 型ごとに1つ

#### 実装状況: ✅ 完了（Block/Handler スコープとして実装）

`handler.rs` の `Handler<K: Kind>` trait で Block 単位のスコープを実装済み:
- `type Scope: Default` で Handler ごとのスコープ型を定義
- `on_event(&mut self, scope: &mut Self::Scope, event: &K::Event)` でスコープ付きイベント処理
- `timeline.rs` の `ErasedHandler` で `start_scope()` / `end_scope()` のライフサイクル管理

#### 計画との差分

- 計画の「Item ctx + Part ctx の二段」ではなく、Block 単位の単一スコープとして実装。
- Block 開始で `Scope::default()` 生成、Block 終了で破棄する方式。Item/Part の入れ子構造は持たない。
- これは十分実用的であり、追加の階層化は必要に応じて将来検討。

### 3. Handler trait の再定義 ✅

- Item/Part を型で指定できる trait を導入する。

#### 実装状況: ✅ 完了（Kind/Handler として実装）

`handler.rs` に以下を実装済み:
- `Kind` trait: イベント型を関連型 `type Event` で指定するマーカー trait
- `Handler<K: Kind>` trait: Kind ごとのイベント処理
- Block Kind 定義:
  - `TextBlockKind` (Event = `TextBlockEvent { Start | Delta | Stop }`)
  - `ThinkingBlockKind` (Event = `ThinkingBlockEvent { Start | Delta | Stop }`)
  - `ToolUseBlockKind` (Event = `ToolUseBlockEvent { Start | InputJsonDelta | Stop }`)
- Meta Kind 定義:
  - `UsageKind`, `PingKind`, `StatusKind`, `ErrorKind`

#### 計画との差分

- 計画の `ItemHandler<I>` / `PartHandler<I, P>` 構造ではなく、`Handler<K: Kind>` の単一 trait で統一。
- PartHandler に ItemCtx を必須で渡す設計は採用せず、Block 単位のフラットな構造。

### 4. Timeline との結合 ✅

- Timeline は BlockStart で Scope を生成、BlockStop で Scope を破棄。

#### 実装状況: ✅ 完了

`timeline/timeline.rs` に以下を実装済み:
- `Timeline` が `ErasedHandler<K>` のリストを保持
- `dispatch(&Event)` でイベントを各 Handler にディスパッチ
- `BlockStart` で `start_scope()`, `BlockStop` / `BlockAbort` で `end_scope()`
- 実用的な collector として `TextBlockCollector`, `ToolCallCollector` を実装済み
- `Worker` が Timeline を所有し、stream ループ内で `timeline.dispatch()` を呼び出し

`subscriber.rs` の `WorkerSubscriber` trait で高レベルのイベント購読を提供:
- `on_text_block()`, `on_tool_use_block()` (スコープ付き)
- `on_usage()`, `on_status()`, `on_error()` (メタイベント)
- `on_text_complete()`, `on_tool_call_complete()` (完了イベント)
- `on_turn_start()`, `on_turn_end()` (ターン制御)

### 5. DSL (macro_rules!) の導入 ⬚

- 宣言的 DSL を提供する。

#### 実装状況: ⬚ 未着手

- `macro_rules!` による `handler!` DSL は未実装。
- 代わりに `llm-worker-macros` クレートで `#[tool_registry]` / `#[tool]` proc-macro を実装済み（ツール定義の自動生成用）。
- Handler 定義の DSL は未提供。現状は trait を直接実装する方式。

#### 次のステップ

- Handler 定義の冗長さが問題になった時点で `handler!` DSL を導入する。
- 現状の `Handler<K>` trait 直接実装 + `WorkerSubscriber` trait の組み合わせで多くのユースケースをカバーできているため、優先度は低い。

### 6. 拡張ポイント 🔄

- 追加 Part (output_image など) を DSL に追加しやすい形にする。

#### 実装状況: 🔄 部分的

- `BlockType` enum に新しい種別を追加し、対応する `Kind` と `Event` を定義すれば拡張可能な設計。
- `ThinkingBlockKind` が後から追加された実績あり。
- proc-macro への移行は未実施（現時点では不要）。

---

## 実装順序（更新版）

1. ~~Event/Item/Part の型定義の整理~~ ✅
2. ~~Item/Part ctx を持つ Timeline 実装~~ ✅ (Block/Scope として)
3. ~~Handler trait の定義・既存コードの移行~~ ✅
4. macro_rules! DSL の実装 ⬚
5. ~~既存ユースケースの移植~~ ✅ (WorkerSubscriber 経由)

---

## TODO（更新版）

- ~~Item と Part の型対応表を整理する~~ ✅ `Item` enum + `BlockType` + `Kind` で対応完了
- ~~OR と既存 llm_client の差分を再確認する~~ ✅ `Item` 型が OR ネイティブに移行済み（Message / FunctionCall / FunctionCallOutput / Reasoning）
- ~~Tool args の delta を OR 拡張として扱うか検討する~~ ✅ `ToolUseBlockEvent::InputJsonDelta` として実装
- macro_rules! で表現可能な DSL の最小文法を確定する ⬚ （優先度低: WorkerSubscriber で大半のユースケースをカバー済み）
