# Hooks 仕様

## 概要

HookはWorker層でのターン制御に介入するためのメカニズムである。

メッセージ送信・ツール実行・ストリーム処理・ターン終了等の各ポイントで処理を差し込むことができる。

## コンセプト

- **制御の介入**: ターンの進行、メッセージの内容、ツールの実行に対して介入
- **Contextへのアクセス**: Item履歴を読み書き可能
- **非破壊的チェーン**: 複数のHookを登録順に実行、後続Hookへの影響を制御

## Hook一覧

| Hook                 | タイミング                 | 主な用途                   | 戻り値                  |
| -------------------- | -------------------------- | -------------------------- | ----------------------- |
| `on_prompt_submit`   | `run()` 呼び出し時         | ユーザーItemの前処理       | `OnPromptSubmitResult`  |
| `pre_llm_request`    | 各ターンのLLM送信前        | コンテキスト改変/検証      | `PreLlmRequestResult`   |
| `pre_tool_call`      | ツール実行前               | 実行許可/引数改変          | `PreToolCallResult`     |
| `post_tool_call`     | ツール実行後               | 結果加工/マスキング        | `PostToolCallResult`    |
| `on_stream_chunk`    | ストリーム各イベント受信後 | 監査/低遅延介入            | `StreamHookResult`      |
| `on_text_delta`      | Text delta受信時           | 出力監視/中断              | `StreamHookResult`      |
| `on_tool_call_delta` | Tool JSON delta受信時      | 引数監視/中断              | `StreamHookResult`      |
| `on_stream_complete` | 1回のstream完了時          | サマリ/完了検証            | `StreamHookResult`      |
| `on_turn_end`        | ツールなしでターン終了直前 | 検証/リトライ指示          | `OnTurnEndResult`       |
| `on_abort`           | 中断時                     | クリーンアップ/通知        | `()`                    |

## Hook Trait

```rust
#[async_trait]
pub trait Hook<E: HookEventKind>: Send + Sync {
    async fn call(&self, input: &mut E::Input) -> Result<E::Output, HookError>;
}
```

## HookEventKind / 制御フロー型

Hookイベントごとに入力型(`Input`)と出力型(`Output`)を分離し、意味のない制御フローを排除する。

```rust
pub trait HookEventKind: Send + Sync + 'static {
    type Input;
    type Output;
}
```

### イベント種別一覧

```rust
pub struct OnPromptSubmit;   // Input: Item,                  Output: OnPromptSubmitResult
pub struct PreLlmRequest;    // Input: Vec<Item>,             Output: PreLlmRequestResult
pub struct PreToolCall;      // Input: ToolCallContext,        Output: PreToolCallResult
pub struct PostToolCall;     // Input: PostToolCallContext,    Output: PostToolCallResult
pub struct OnTurnEnd;        // Input: Vec<Item>,             Output: OnTurnEndResult
pub struct OnAbort;          // Input: String,                Output: ()
pub struct OnTextDelta;      // Input: TextDeltaContext,      Output: StreamHookResult
pub struct OnToolCallDelta;  // Input: ToolCallDeltaContext,  Output: StreamHookResult
pub struct OnStreamChunk;    // Input: StreamChunkContext,    Output: StreamHookResult
pub struct OnStreamComplete; // Input: StreamCompleteContext, Output: StreamHookResult
```

### 制御フロー Result 型

```rust
pub enum OnPromptSubmitResult {
    Continue,
    Cancel(String),
}

pub enum PreLlmRequestResult {
    Continue,
    Cancel(String),
}

pub enum PreToolCallResult {
    Continue,
    Skip,
    Abort(String),
    Pause,
}

pub enum PostToolCallResult {
    Continue,
    Abort(String),
}

pub enum OnTurnEndResult {
    Finish,
    ContinueWithMessages(Vec<Item>),
    Paused,
}

pub enum StreamHookResult {
    Continue,
    Abort(String),
    Pause,
}
```

### コンテキスト型

#### ToolCallContext (PreToolCall用)

```rust
pub struct ToolCallContext {
    pub call: ToolCall,       // ツール呼び出し情報（改変可能）
    pub meta: ToolMeta,       // 不変メタデータ
    pub tool: Arc<dyn Tool>,  // 状態アクセス用
}
```

#### PostToolCallContext (PostToolCall用)

```rust
pub struct PostToolCallContext {
    pub call: ToolCall,       // ツール呼び出し情報
    pub result: ToolResult,   // 実行結果（改変可能）
    pub meta: ToolMeta,       // 不変メタデータ
    pub tool: Arc<dyn Tool>,  // 状態アクセス用
}
```

#### TextDeltaContext (OnTextDelta用)

```rust
pub struct TextDeltaContext {
    pub index: usize,    // ブロックインデックス
    pub delta: String,   // テキストデルタ内容
}
```

#### ToolCallDeltaContext (OnToolCallDelta用)

```rust
pub struct ToolCallDeltaContext {
    pub index: usize,                 // ブロックインデックス
    pub delta_json_fragment: String,  // 部分JSONフラグメント
}
```

#### StreamChunkContext (OnStreamChunk用)

```rust
pub struct StreamChunkContext {
    pub event: crate::event::Event,   // Worker層の公開イベント
}
```

#### StreamCompleteContext (OnStreamComplete用)

```rust
pub struct StreamCompleteContext {
    pub turn: usize,         // 現在のターン番号
    pub event_count: usize,  // このリクエストでのストリームイベント数
}
```

## 呼び出しタイミング

```
Worker::run(user_input)
│
├── on_prompt_submit ─────────────────────────┐
│   ユーザーItemの前処理・検証              │
│   （最初の1回のみ）                        │
│                                            │
└── loop {
    │
    ├── pre_llm_request ──────────────────│
    │   history の clone に対して改変・検証   │
    │   （毎ターン実行）                      │
    │                                        │
    ├── LLMリクエスト送信 & ストリーム処理    │
    │   │                                    │
    │   ├── on_stream_chunk (各イベント)      │
    │   ├── on_text_delta (テキストデルタ)    │
    │   └── on_tool_call_delta (JSONデルタ)   │
    │                                        │
    ├── on_stream_complete                   │
    │                                        │
    ├── ツール呼び出しがある場合:              │
    │   │                                    │
    │   ├── pre_tool_call (各ツールごと・逐次) │
    │   │   実行可否の判定、引数の改変         │
    │   │                                    │
    │   ├── ツール並列実行 (join_all)          │
    │   │                                    │
    │   └── post_tool_call (各結果ごと・逐次)  │
    │       結果の確認、加工、ログ出力           │
    │                                        │
    ├── ツール結果をhistoryに追加              │
    │   → ループ先頭へ                         │
    │                                        │
    └── ツールなしの場合:                     │
        │                                    │
        └── on_turn_end  ───────────────┘
            最終応答のチェック（Lint/Fmt等）
            エラーがあればContinueWithMessagesでリトライ
}

※ 中断時は on_abort が呼ばれる（finalize_interruption経由）
```

## 各Hookの詳細

### on_prompt_submit

**呼び出しタイミング**: `run()` でユーザーメッセージを受け取った直後（最初の1回のみ）

**入力**: `&mut Item` - ユーザーItem（改変可能）

**用途**:

- ユーザー入力のバリデーション
- 入力のサニタイズ・フィルタリング
- ログ出力
- `OnPromptSubmitResult::Cancel` による実行キャンセル（`WorkerError::Aborted`になる）

**例**: 入力のバリデーション

```rust
struct InputValidator;

#[async_trait]
impl Hook<OnPromptSubmit> for InputValidator {
    async fn call(
        &self,
        item: &mut Item,
    ) -> Result<OnPromptSubmitResult, HookError> {
        if let Item::Message { content, .. } = item {
            if content.trim().is_empty() {
                return Ok(OnPromptSubmitResult::Cancel("Empty input".to_string()));
            }
        }
        Ok(OnPromptSubmitResult::Continue)
    }
}
```

### pre_llm_request

**呼び出しタイミング**: 各ターンのLLMリクエスト送信前（ループの毎回）

**入力**: `&mut Vec<Item>` - historyのclone（改変可能、元のhistoryは変更されない）

**用途**:

- コンテキストへのシステムメッセージ注入
- Itemのバリデーション
- 機密情報のフィルタリング
- リクエスト内容のログ出力
- `PreLlmRequestResult::Cancel` による送信キャンセル（`WorkerError::Aborted`になる）

### pre_tool_call

**呼び出しタイミング**: 各ツール実行前（並列実行フェーズの前に逐次実行）

**入力**: `&mut ToolCallContext`（`ToolCall` + `ToolMeta` + `Arc<dyn Tool>`）

**用途**:

- 危険なツールのブロック（`Skip`）
- 引数のサニタイズ（`context.call.input` の直接改変）
- 確認プロンプトの表示（UIとの連携）
- 実行ログの記録
- `PreToolCallResult::Pause` による一時停止（`WorkerResult::Paused`を返す）
- `PreToolCallResult::Abort` による処理全体の中断

**備考**: 未登録ツール（ToolServerに存在しないツール名）の場合、Hookは適用されずそのまま実行される（実行時にエラーとなる）。

**例**: 特定ツールをブロック

```rust
struct ToolBlocker {
    blocked_tools: HashSet<String>,
}

#[async_trait]
impl Hook<PreToolCall> for ToolBlocker {
    async fn call(
        &self,
        ctx: &mut ToolCallContext,
    ) -> Result<PreToolCallResult, HookError> {
        if self.blocked_tools.contains(&ctx.call.name) {
            Ok(PreToolCallResult::Skip)
        } else {
            Ok(PreToolCallResult::Continue)
        }
    }
}
```

### post_tool_call

**呼び出しタイミング**: 各ツール実行後（並列実行フェーズの後に逐次実行）

**入力**: `&mut PostToolCallContext`（`ToolCall` + `ToolResult` + `ToolMeta` + `Arc<dyn Tool>`）

**用途**:

- 結果の加工・フォーマット（`context.result` の直接改変）
- 機密情報のマスキング
- 結果のキャッシュ
- 実行結果のログ出力
- `PostToolCallResult::Abort` による処理全体の中断

**例**: 結果にプレフィックスを追加

```rust
struct ResultFormatter;

#[async_trait]
impl Hook<PostToolCall> for ResultFormatter {
    async fn call(
        &self,
        ctx: &mut PostToolCallContext,
    ) -> Result<PostToolCallResult, HookError> {
        if !ctx.result.is_error {
            ctx.result.content = format!("[OK] {}", ctx.result.content);
        }
        Ok(PostToolCallResult::Continue)
    }
}
```

### on_stream_chunk

**呼び出しタイミング**: Timeline dispatchの後、各ストリームイベント受信時

**入力**: `&mut StreamChunkContext`（Worker層の`Event`を含む）

**用途**:

- 監査・ログ記録
- 低遅延での介入

### on_text_delta

**呼び出しタイミング**: `BlockDelta`イベントのうち`DeltaContent::Text`受信時

**入力**: `&mut TextDeltaContext`（`index` + `delta`）

**用途**:

- テキスト出力の監視
- 特定パターン検出による中断

### on_tool_call_delta

**呼び出しタイミング**: `BlockDelta`イベントのうち`DeltaContent::InputJson`受信時

**入力**: `&mut ToolCallDeltaContext`（`index` + `delta_json_fragment`）

**用途**:

- ツール引数JSONの部分監視
- 危険な引数パターン検出による中断

### on_stream_complete

**呼び出しタイミング**: 1回のストリームが完了した後（内側ループ終了後）

**入力**: `&mut StreamCompleteContext`（`turn` + `event_count`）

**用途**:

- ストリーム完了の検証
- イベント数の記録

### on_turn_end

**呼び出しタイミング**: ツール呼び出しなしでターンが終了する直前

**入力**: `&mut Vec<Item>` - historyのclone（改変可能）

**用途**:

- 生成されたコードのLint/Fmt
- 出力形式のバリデーション
- 自己修正のためのリトライ指示（`ContinueWithMessages`でItemを追加して再ループ）
- 最終結果のログ出力
- `OnTurnEndResult::Paused` による一時停止

**例**: JSON形式のバリデーション

```rust
struct JsonValidator;

#[async_trait]
impl Hook<OnTurnEnd> for JsonValidator {
    async fn call(
        &self,
        items: &mut Vec<Item>,
    ) -> Result<OnTurnEndResult, HookError> {
        // 最後のアシスタントメッセージを検査
        let last_text = items.iter().rev().find_map(|item| {
            if let Item::Message { role, content, .. } = item {
                if role == "assistant" { Some(content.as_str()) } else { None }
            } else { None }
        });

        if let Some(text) = last_text {
            if serde_json::from_str::<serde_json::Value>(text).is_err() {
                return Ok(OnTurnEndResult::ContinueWithMessages(vec![
                    Item::user_message("Invalid JSON. Please fix and try again.")
                ]));
            }
        }
        Ok(OnTurnEndResult::Finish)
    }
}
```

### on_abort

**呼び出しタイミング**: Worker実行が中断された時（`finalize_interruption`経由）

**入力**: `&mut String` - 中断理由

**用途**:

- クリーンアップ処理
- 中断理由のログ出力
- 外部システムへの通知

**発火条件**: `run()`や`resume()`の結果が`Err`の場合に必ず発火する。具体的には以下のケース:

- `WorkerError::Cancelled` -- reason: `"Cancelled"`
- `WorkerError::Aborted(reason)` -- reason: フックやキャンセルが指定した理由
- その他のエラー（Client, Tool, Hook等） -- reason: エラーの表示文字列

## ストリームイベントの処理順序

`BlockDelta`到着時の順序:

1. Timelineへdispatch（collector/subscriber更新）
2. `on_stream_chunk`
3. `on_text_delta` または `on_tool_call_delta`（デルタ種別による）

※ `DeltaContent::Thinking` に対するHookは現在存在しない。

## 複数Hookの実行順序

Hookは**イベントごとに登録順**に実行される。

```rust
worker.add_pre_tool_call_hook(HookA);  // 1番目に実行
worker.add_pre_tool_call_hook(HookB);  // 2番目に実行
worker.add_pre_tool_call_hook(HookC);  // 3番目に実行
```

### 制御フローの伝播

- `Continue` / `Finish`: 後続Hookも実行
- `Skip`: 現在の処理をスキップし、後続Hookは実行しない
- `Abort`: 即座に`WorkerError::Aborted`を返し、処理全体を中断
- `Pause`: Workerを一時停止（`WorkerResult::Paused`を返す。`resume()`で再開可能）
- `Cancel`: `WorkerError::Aborted`を返し、処理を中断

```
Hook A: Continue → Hook B: Skip → (Hook Cは実行されない)
                                   ↓
                            処理をスキップ

Hook A: Continue → Hook B: Abort("reason")
                           ↓
                     WorkerError::Aborted

Hook A: Continue → Hook B: Pause
                           ↓
                     WorkerResult::Paused
```

注: stream系Hookの`Pause`は現状 `WorkerError::Aborted("Paused by stream hook")` として扱われる。

## Hook登録API

Workerは各Hookイベントに対応する登録メソッドを提供する:

```rust
worker.add_on_prompt_submit_hook(hook);
worker.add_pre_llm_request_hook(hook);
worker.add_pre_tool_call_hook(hook);
worker.add_post_tool_call_hook(hook);
worker.add_on_turn_end_hook(hook);
worker.add_on_abort_hook(hook);
worker.add_on_text_delta_hook(hook);
worker.add_on_tool_call_delta_hook(hook);
worker.add_on_stream_chunk_hook(hook);
worker.add_on_stream_complete_hook(hook);
```

これらのメソッドはWorkerの状態（`Mutable`/`CacheLocked`）に関係なく利用可能。

## HookRegistry

全Hookは`HookRegistry`構造体で内部管理される。各フィールドは`Vec<Box<dyn Hook<E>>>`であり、Workerが初期化時に空のレジストリを作成する。

## HookError

```rust
pub enum HookError {
    Aborted(String),     // 処理の中断
    Internal(String),    // 内部エラー
}
```

`HookError`は`WorkerError::Hook`に変換され、`finalize_interruption`で`on_abort`フックが発火する。

## 設計上のポイント

### 1. イベントごとの実装

必要なイベントのみ `Hook<Event>` を実装する。1つの構造体で複数イベントの`Hook`を実装可能。

### 2. 可変参照による改変

`&mut`で引数を受け取るため、直接改変が可能。

```rust
async fn call(&self, ctx: &mut ToolCallContext) -> ... {
    ctx.call.input["sanitized"] = json!(true);
    Ok(PreToolCallResult::Continue)
}
```

### 3. 並列実行との統合

- `pre_tool_call`: 並列実行**前**に逐次実行（許可判定のため）
- ツール実行: `join_all`で**並列**実行（`tokio::select!`によりキャンセル可能）
- `post_tool_call`: 並列実行**後**に逐次実行（結果加工のため）

### 4. Send + Sync 要件

`Hook`は`Send + Sync`を要求するため、スレッドセーフな実装が必要。
状態を持つ場合は`Arc<Mutex<T>>`や`AtomicUsize`などを使用する。

### 5. pre_llm_request / on_turn_end のコンテキスト

`pre_llm_request`と`on_turn_end`はhistoryの**clone**に対して操作する。Hookによる改変はリクエスト構築時またはリトライ用メッセージ追加時のみ反映され、Worker内部のhistoryは直接変更されない。

## 典型的なユースケース

| ユースケース       | 使用Hook               | 処理内容                   |
| ------------------ | ---------------------- | -------------------------- |
| ツール許可制御     | `pre_tool_call`        | 危険なツールをSkip         |
| 実行ログ           | `pre/post_tool_call`   | 呼び出しと結果を記録       |
| 出力バリデーション | `on_turn_end`          | 形式チェック、リトライ指示 |
| コンテキスト注入   | `pre_llm_request`      | システムメッセージ追加     |
| 結果のサニタイズ   | `post_tool_call`       | 機密情報のマスキング       |
| レート制限         | `pre_tool_call`        | 呼び出し頻度の制御         |
| ストリーム監視     | `on_text_delta`        | 出力内容のリアルタイム監視 |
| 中断時通知         | `on_abort`             | 外部システムへの通知       |
