# Worker 設計

## 概要

`Worker`はアプリケーションの「ターン」を制御する高レベルコンポーネントです。
`LlmClient`と`Timeline`を内包し、ユーザー定義の`Tool`と`Hook`を用いて自律的なインタラクションを行います。

Type-stateパターンにより、`Mutable`（編集可能）と`CacheLocked`（キャッシュ保護）の2つの状態を持ちます。

## アーキテクチャ

```mermaid
graph TD
    User[Application / User] -->|1. Run| Worker
    Worker -->|2. Event Loop| Timeline
    Timeline -->|3. Dispatch| Handler[Handlers (TextBlockCollector, ToolCallCollector)]

    subgraph "Worker Layer"
        Worker
        Hook[HookRegistry]
        ToolServer[ToolServerHandle]
    end

    subgraph "Core Layer"
        Timeline
        LlmClient
    end

    Worker -.->|Intervene| Hook
    ToolServer -.->|Execute| Tool[User Defined Tools]
    Worker -.->|Subscribe| Subscriber[WorkerSubscriber]
```

## Worker 構造体

```rust
pub struct Worker<C: LlmClient, S: WorkerState = Mutable> {
    client: C,
    timeline: Timeline,
    text_block_collector: TextBlockCollector,
    tool_call_collector: ToolCallCollector,
    tool_server: ToolServerHandle,
    hooks: HookRegistry,
    system_prompt: Option<String>,
    history: Vec<Item>,
    locked_prefix_len: usize,
    turn_count: usize,
    turn_notifiers: Vec<Box<dyn TurnNotifier>>,
    request_config: RequestConfig,
    last_run_interrupted: bool,
    cancel_tx: mpsc::Sender<()>,
    cancel_rx: mpsc::Receiver<()>,
    _state: PhantomData<S>,
}
```

## 状態遷移 (Type-state)

| 状態 | 説明 | 利用可能な操作 |
| --- | --- | --- |
| `Mutable` | 初期状態。自由に編集可能 | system_prompt設定、history編集、tool/hook登録、lock() |
| `CacheLocked` | キャッシュ保護状態 | historyへの追記のみ、unlock() |

```rust
// Mutable → CacheLocked
let locked = worker.lock();

// CacheLocked → Mutable
let mutable = locked.unlock();
```

`CacheLocked`状態ではLLM APIのKVキャッシュヒットを確保するため、
システムプロンプトと既存の履歴（ロック時点のプレフィックス）は不変となります。

## ライフサイクル (ターン制御)

Workerは以下のループ（ターン）を実行します。

1. **Start Turn**: `Worker::run(user_input)` 呼び出し
2. **Hook: OnPromptSubmit**:
   - ユーザーメッセージ(`Item`)の改変、バリデーション、キャンセルが可能。
   - `Cancel` を返すと `WorkerError::Aborted` で終了。
3. **Resume Check**:
   - 未応答のToolCall（Pauseから復帰した場合等）があれば、先にツール実行を行う。
4. **Turn Loop**:
   1. **Hook: PreLlmRequest**:
      - LLMリクエスト送信前にコンテキスト（`Vec<Item>`）を改変可能。
      - `Cancel` を返すとターン中断。
   2. **Request & Stream**:
      - LLMへリクエスト送信。イベントストリーム開始。
      - `Timeline`によるイベント処理（`TextBlockCollector`, `ToolCallCollector`）。
      - ストリーム中に **Hook: OnStreamChunk**, **OnTextDelta**, **OnToolCallDelta** を実行。
      - ストリーム完了時に **Hook: OnStreamComplete** を実行。
   3. **Tool Handling (Parallel)**:
      - レスポンス内に含まれる全てのTool Callを収集。
      - 各Toolに対して **Hook: PreToolCall** を実行（実行可否、引数改変、Pause）。
      - 許可されたToolを**並列実行 (`join_all`)**（`ToolServerHandle`経由）。
      - 各Tool実行後に **Hook: PostToolCall** を実行（結果の確認、加工）。
   4. **Next Request Decision**:
      - Tool実行結果がある場合 -> 結果を`Item::FunctionCallOutput`としてhistoryに追加し、**Step 4.1へ戻る**（自動ループ）。
      - Tool実行がない場合 -> Step 5へ。
5. **Hook: OnTurnEnd**:
   - 最終的な応答に対するチェック（Lint/Fmtなど）。
   - `ContinueWithMessages(items)`: 追加メッセージをhistoryに追加して**Step 4.1へ戻る**ことで自己修正を促せる。
   - `Paused`: `WorkerResult::Paused` を返し、後で `resume()` で再開可能。
   - `Finish`: ターン正常終了。

## キャンセル機構

`cancel_tx` / `cancel_rx` チャネルによる非同期キャンセルをサポートします。

```rust
// キャンセルトリガーの取得
let sender = worker.cancel_sender();

// 別タスクからキャンセル
sender.try_send(()).ok();

// または直接
worker.cancel();
```

キャンセルはストリーム開始前、ストリーム受信中、ツール実行中のいずれのタイミングでも
`tokio::select!` により検出され、`WorkerError::Cancelled` を返します。

## 実行結果

```rust
pub enum WorkerResult {
    /// 完了（ユーザー入力待ち）
    Finished,
    /// 一時停止（resume()で再開可能）
    Paused,
}
```

`resume()` メソッドにより、Paused状態からユーザーメッセージを追加せずにターン処理を再開できます。

## Hook 設計

### Hook Trait

```rust
#[async_trait]
pub trait Hook<E: HookEventKind>: Send + Sync {
    async fn call(&self, input: &mut E::Input) -> Result<E::Output, HookError>;
}

pub trait HookEventKind: Send + Sync + 'static {
    type Input;
    type Output;
}
```

### Hook イベント一覧

| Hook | Input | Output | タイミング |
| --- | --- | --- | --- |
| `OnPromptSubmit` | `Item` | `OnPromptSubmitResult` | `run()` 直後、ユーザーメッセージ受信時 |
| `PreLlmRequest` | `Vec<Item>` | `PreLlmRequestResult` | 各ターンのLLMリクエスト送信前 |
| `PreToolCall` | `ToolCallContext` | `PreToolCallResult` | 各ツール実行前 |
| `PostToolCall` | `PostToolCallContext` | `PostToolCallResult` | 各ツール実行後 |
| `OnTurnEnd` | `Vec<Item>` | `OnTurnEndResult` | ツール呼び出しなしでターン終了時 |
| `OnAbort` | `String` | `()` | エラーまたはキャンセルで中断時 |
| `OnTextDelta` | `TextDeltaContext` | `StreamHookResult` | テキストデルタ受信時 |
| `OnToolCallDelta` | `ToolCallDeltaContext` | `StreamHookResult` | ツール呼び出しJSON断片受信時 |
| `OnStreamChunk` | `StreamChunkContext` | `StreamHookResult` | ストリームイベント受信時 |
| `OnStreamComplete` | `StreamCompleteContext` | `StreamHookResult` | ストリーム完了時 |

### Hook 結果型

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
    Skip,          // ツール実行をスキップ
    Abort(String), // 処理中断
    Pause,         // 一時停止（resume()で再開）
}

pub enum PostToolCallResult {
    Continue,
    Abort(String),
}

pub enum OnTurnEndResult {
    Finish,
    ContinueWithMessages(Vec<Item>), // メッセージを追加してターン継続
    Paused,
}

pub enum StreamHookResult {
    Continue,
    Abort(String),
    Pause,
}
```

### Tool Call Context

`PreToolCall` / `PostToolCall` は、ツール実行の文脈を含むコンテキストを受け取ります。

```rust
pub struct ToolCallContext {
    pub call: ToolCall,      // 呼び出し情報（改変可能）
    pub meta: ToolMeta,      // メタ情報（読み取り専用）
    pub tool: Arc<dyn Tool>, // インスタンス（状態アクセス用）
}

pub struct PostToolCallContext {
    pub call: ToolCall,      // 呼び出し情報
    pub result: ToolResult,  // 実行結果（改変可能）
    pub meta: ToolMeta,      // メタ情報（読み取り専用）
    pub tool: Arc<dyn Tool>, // インスタンス（状態アクセス用）
}
```

### ストリーミングHookのコンテキスト

```rust
pub struct TextDeltaContext {
    pub index: usize,
    pub delta: String,
}

pub struct ToolCallDeltaContext {
    pub index: usize,
    pub delta_json_fragment: String,
}

pub struct StreamChunkContext {
    pub event: crate::event::Event,
}

pub struct StreamCompleteContext {
    pub turn: usize,
    pub event_count: usize,
}
```

### Hook 登録API

```rust
worker.add_on_prompt_submit_hook(my_hook);
worker.add_pre_llm_request_hook(my_hook);
worker.add_pre_tool_call_hook(my_hook);
worker.add_post_tool_call_hook(my_hook);
worker.add_on_turn_end_hook(my_hook);
worker.add_on_abort_hook(my_hook);
worker.add_on_text_delta_hook(my_hook);
worker.add_on_tool_call_delta_hook(my_hook);
worker.add_on_stream_chunk_hook(my_hook);
worker.add_on_stream_complete_hook(my_hook);
```

## Worker Event API (Subscriber)

### 背景と目的

Workerは内部でイベントを処理し結果を返しますが、UIへのストリーミング表示やリアルタイムフィードバックには、イベントを外部に公開する仕組みが必要です。

### WorkerSubscriber Trait

`WorkerSubscriber`トレイトを実装し、`worker.subscribe()` で一括登録します。

```rust
pub trait WorkerSubscriber: Send {
    // スコープ型（ブロックイベント用）
    type TextBlockScope: Default + Send + Sync;
    type ToolUseBlockScope: Default + Send + Sync;

    // === ブロックイベント（スコープ管理あり）===
    fn on_text_block(
        &mut self,
        scope: &mut Self::TextBlockScope,
        event: &TextBlockEvent,
    ) {}

    fn on_tool_use_block(
        &mut self,
        scope: &mut Self::ToolUseBlockScope,
        event: &ToolUseBlockEvent,
    ) {}

    // === 単発イベント ===
    fn on_usage(&mut self, event: &UsageEvent) {}
    fn on_status(&mut self, event: &StatusEvent) {}
    fn on_error(&mut self, event: &ErrorEvent) {}

    // === 累積イベント（Worker層で追加）===
    fn on_text_complete(&mut self, text: &str) {}
    fn on_tool_call_complete(&mut self, call: &ToolCall) {}

    // === ターン制御 ===
    fn on_turn_start(&mut self, turn: usize) {}
    fn on_turn_end(&mut self, turn: usize) {}
}
```

### 内部実装

SubscriberはWorker内部でAdapter群に分解され、各KindのHandlerとしてTimelineに登録されます。
累積イベント（`on_text_complete`, `on_tool_call_complete`）はAdapter内でブロック終了時にバッファから合成されます。

- `TextBlockSubscriberAdapter`: テキストデルタを蓄積し、Stop時に`on_text_complete`を呼ぶ
- `ToolUseBlockSubscriberAdapter`: ID・名前・JSONを蓄積し、Stop時に`on_tool_call_complete`を呼ぶ
- `UsageSubscriberAdapter`, `StatusSubscriberAdapter`, `ErrorSubscriberAdapter`: 単純な転送
- `SubscriberTurnNotifier`: ターン開始・終了の通知

### 使用例

```rust
struct StreamPrinter;

impl WorkerSubscriber for StreamPrinter {
    type TextBlockScope = ();
    type ToolUseBlockScope = ();

    fn on_text_block(&mut self, _: &mut (), event: &TextBlockEvent) {
        if let TextBlockEvent::Delta(text) = event {
            print!("{}", text);
        }
    }

    fn on_text_complete(&mut self, text: &str) {
        println!("\n--- Complete: {} chars ---", text.len());
    }
}

worker.subscribe(StreamPrinter);
let result = worker.run("Hello!").await?;
```

## Worker API

### 生成と設定 (Mutable状態のみ)

```rust
// 生成（ビルダーパターン）
let worker = Worker::new(client)
    .system_prompt("You are a helpful assistant.")
    .max_tokens(4096)
    .temperature(0.7)
    .top_p(0.9)
    .top_k(40)
    .stop_sequence("\n\n")
    .with_config(request_config)
    .with_item(item)
    .with_items(items);

// バリデーション（プロバイダの対応確認）
let worker = worker.validate()?;

// ミューテーション
worker.set_system_prompt("...");
worker.set_max_tokens(4096);
worker.set_temperature(0.7);
worker.set_top_p(0.9);
worker.set_top_k(40);
worker.add_stop_sequence("\n\n");
worker.clear_stop_sequences();
worker.set_request_config(config);

// 履歴操作
worker.push_item(item);
worker.extend_history(items);
worker.set_history(items);
worker.clear_history();
worker.history_mut();  // &mut Vec<Item>

// ツール登録
worker.register_tool(factory)?;
worker.register_tools(factories)?;
```

### 全状態で利用可能

```rust
// 実行
let result = worker.run("user input").await?;
let result = worker.resume().await?;

// キャンセル
worker.cancel();
let sender = worker.cancel_sender();

// 参照
worker.history();           // &[Item]
worker.get_system_prompt();  // Option<&str>
worker.turn_count();         // usize
worker.request_config();     // &RequestConfig
worker.last_run_interrupted(); // bool
worker.is_cancelled();       // bool

// ツールサーバー
worker.tool_server_handle(); // ToolServerHandle

// Timeline直接アクセス
worker.timeline_mut();       // &mut Timeline

// イベント購読
worker.subscribe(my_subscriber);

// Hook登録
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

## エラー型

```rust
pub enum WorkerError {
    Client(ClientError),
    Tool(ToolError),
    Hook(HookError),
    Aborted(String),
    Cancelled,
    ConfigWarnings(Vec<ConfigWarning>),
}

pub enum ToolRegistryError {
    DuplicateName(String),
}
```

## 公開エクスポート

`lib.rs` から以下がre-exportされています。

```rust
pub use message::{ContentPart, Item, Message, Role};
pub use worker::{ToolRegistryError, Worker, WorkerConfig, WorkerError, WorkerResult};
```

モジュール:
- `pub mod event`
- `pub mod hook`
- `pub mod llm_client`
- `pub mod state`
- `pub mod subscriber`
- `pub mod timeline`
- `pub mod tool`
- `pub mod tool_server`
