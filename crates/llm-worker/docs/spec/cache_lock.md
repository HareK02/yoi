# KVキャッシュを中心とした設計

LLMのKVキャッシュのヒット率を重要なメトリクスであるとし、APIレベルでキャッシュ操作を中心とした設計を行う。

## 前提

リクエスト間キャッシュ(Context Caching)は、複数のリクエストで同じ入力トークン列が繰り返された際、プロバイダ側が計算済みの状態を再利用することでレイテンシと入力コストを下げる仕組みである。
キャッシュは主に**先頭一致 (Common Prefix)** によってHitするため、前提となるシステムプロンプトや、会話ログの過去部分（前方）を変化させると、以降のキャッシュは無効となる。

## 要件

1.  **前方不変性の保証 (Prefix Immutability)**
    *   後方に会話が追加されても、前方のデータ（システムプロンプトや確定済みのメッセージ履歴）が変化しないことをAPIレベルで保証する。
    *   これにより、意図しないキャッシュミス（Cache Miss）を防ぐ。

2.  **データ上の再現性**
    *   コンテキストのデータ構造が同一であれば、生成されるリクエスト構造も同一であることを保証する。
    *   シリアライズ結果のバイト単位の完全一致までは求めないが、論理的なリクエスト構造は保たれる必要がある。

## アプローチ: Type-state Pattern

RustのType-stateパターンを利用し、Workerの状態によって利用可能な操作をコンパイル時に制限する。

### 1. 状態定義（`state.rs`）

`WorkerState`トレイトはsealedパターンで実装され、外部からの実装を防ぐ:

```rust
pub trait WorkerState: private::Sealed + Send + Sync + 'static {}

mod private {
    pub trait Sealed {}
}
```

*   **`Mutable` (初期状態)**
    *   自由な編集が可能な状態。
    *   システムプロンプトの設定・変更が可能。
    *   メッセージ履歴の初期構築（ロード、編集、クリア）が可能。
    *   ツール・フックの登録が可能。
    *   リクエスト設定（`max_tokens`, `temperature`, `top_p`, `top_k`, `stop_sequences`）の変更が可能。
*   **`CacheLocked` (キャッシュ保護状態)**
    *   キャッシュの有効活用を目的とした、前方不変状態。
    *   **システムプロンプトの変更不可**。
    *   **既存メッセージ履歴の変更不可**（`run()`による末尾追記のみ許可）。
    *   実行（`run`）はこの状態で行うことを推奨する。

両状態とも`Debug`, `Clone`, `Copy`, `Default`を実装する。

### 2. Worker構造体

`Worker`は状態パラメータ`S: WorkerState`を持ち、デフォルトは`Mutable`:

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
    locked_prefix_len: usize,       // ロック時の履歴長
    turn_count: usize,
    turn_notifiers: Vec<Box<dyn TurnNotifier>>,
    request_config: RequestConfig,
    last_run_interrupted: bool,
    cancel_tx: mpsc::Sender<()>,
    cancel_rx: mpsc::Receiver<()>,
    _state: PhantomData<S>,
}
```

`Worker`自身がコンテキスト（履歴）のオーナーとなり、状態によってアクセサを制限する。

### 3. 状態遷移とAPI

#### 共通API（全状態で利用可能）

```rust
impl<C: LlmClient, S: WorkerState> Worker<C, S> {
    pub async fn run(&mut self, user_input: impl Into<String>) -> Result<WorkerResult, WorkerError>;
    pub fn history(&self) -> &[Item];         // 参照のみ
    pub fn system_prompt(&self) -> Option<&str>;
    pub fn cancel_token(&self) -> CancelToken; // キャンセルトークン取得
    // ... その他参照系メソッド
}
```

#### Mutable限定API

```rust
impl<C: LlmClient> Worker<C, Mutable> {
    pub fn new(client: C) -> Self;

    // システムプロンプト
    pub fn system_prompt(self, prompt: impl Into<String>) -> Self;  // ビルダー
    pub fn set_system_prompt(&mut self, prompt: impl Into<String>);

    // 履歴操作
    pub fn history_mut(&mut self) -> &mut Vec<Item>;
    pub fn set_history(&mut self, items: Vec<Item>);
    pub fn with_item(self, item: Item) -> Self;
    pub fn push_item(&mut self, item: Item);
    pub fn with_items(self, items: impl IntoIterator<Item = Item>) -> Self;
    pub fn extend_history(&mut self, items: impl IntoIterator<Item = Item>);
    pub fn clear_history(&mut self);

    // ツール登録
    pub fn register_tool(&mut self, factory: WorkerToolDefinition) -> Result<(), ToolRegistryError>;
    pub fn register_tools(&mut self, factories: impl IntoIterator<Item = WorkerToolDefinition>) -> Result<(), ToolRegistryError>;

    // リクエスト設定
    pub fn max_tokens(self, max_tokens: u32) -> Self;
    pub fn temperature(self, temperature: f32) -> Self;
    pub fn top_p(self, top_p: f32) -> Self;
    pub fn top_k(self, top_k: u32) -> Self;
    pub fn stop_sequence(self, sequence: impl Into<String>) -> Self;
    pub fn with_config(self, config: RequestConfig) -> Self;
    pub fn validate(self) -> Result<Self, WorkerError>;

    // 状態遷移
    pub fn lock(self) -> Worker<C, CacheLocked>;
}
```

#### CacheLocked限定API

```rust
impl<C: LlmClient> Worker<C, CacheLocked> {
    pub fn locked_prefix_len(&self) -> usize;
    pub fn unlock(self) -> Worker<C, Mutable>;
}
```

### 4. 使用例

```rust
// 1. Mutable状態で初期化
let mut worker = Worker::new(client)
    .system_prompt("You are a helpful assistant.")
    .max_tokens(4096);

// 2. コンテキストの構築（Mutableなので自由に変更可）
worker.push_item(Item::user_message("Hello"));
worker.register_tool(my_tool)?;

// 3. ロックしてCacheLocked状態へ遷移
//    ここまでの履歴長がlocked_prefix_lenとして記録される
let mut locked_worker = worker.lock();

// 4. 利用（CacheLocked状態）
// 実行は可能。新しいメッセージは履歴の末尾に追記される。
// 前方の履歴やシステムプロンプトは変更できないため、キャッシュヒットが保証される。
locked_worker.run("user input").await?;

// NG操作（コンパイルエラー）
// locked_worker.set_system_prompt("New prompt");
// locked_worker.history_mut().clear();
// locked_worker.register_tool(another_tool);

// 5. 必要に応じてアンロック（キャッシュ保護は解除される）
let mut mutable_worker = locked_worker.unlock();
mutable_worker.set_system_prompt("New prompt");
```

### 5. lock/unlockの実装

`lock()`と`unlock()`はWorkerの全フィールドを移動して新しい状態のWorkerを構築する。値の所有権移動のため、元のWorkerは使用不可になる。

- `lock()`: `locked_prefix_len`に現在の`history.len()`を記録
- `unlock()`: `locked_prefix_len`を0にリセット

### 6. UsageEventによるキャッシュ監視

`UsageEvent`には`cache_read_input_tokens`と`cache_creation_input_tokens`が含まれており、キャッシュヒット率のモニタリングが可能:

```rust
pub struct UsageEvent {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}
```

TimelineのUsageKind Handlerを登録することで、キャッシュの効果を実行時に確認できる。
