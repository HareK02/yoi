# Timeline層設計

## 目的

- OpenAI / Anthropic / Gemini のストリーミングイベントを単一の抽象レイヤーに正規化し、LLMクライアントの状態遷移を制御する。
- イベント単位処理（Meta系など）とブロック単位処理（テキスト/Thinking/ToolCall）を同一のパイプラインで扱えるようにする。

## 要件

イベントストリームに対して直接的にループ処理をしようとすると発生する煩雑な状態管理を避ける。

イベントをloop+matchで処理をするような手段を取ると、テキストのデルタの更新先や、完了タイミングなどの状態管理が必要になる。
また、コンテンツブロックに対する単純なイテレータではping/usageなどの単発イベントを同期的に処理することができない。

- Meta系イベントの即時処理（ブロック内部イベントと順序が前後しないようにする）
- ブロック開始/差分/終了でスコープを保持する
- 型安全なハンドラー
  - blockでキャッチするdeltaについて、Text/InputJson/Thinking等、ブロックに即したイベントの型が必要。
- エラーの適切な制御

## Memo

- ブロックを処理するHandlerで保持するコンテキストについて、LLMで用いられるコンテキストと混同を避けるために「スコープ」と呼称する。
- ブロックは常に一つである前提。複数のブロックが同時に存在することは無いため。`Timeline`は`current_block: Option<BlockType>`で現在のブロックを追跡する。

## モジュール構成

```
crates/llm-worker/src/
├── handler.rs           # Kind / Handler トレイト定義、Meta系・Block系Kind定義
├── timeline/
│   ├── mod.rs           # 公開API re-export
│   ├── event.rs         # Timeline層のイベント型（llm_client::eventからの変換含む）
│   ├── timeline.rs      # Timeline本体、ErasedHandler、BlockHandlerWrapper群
│   ├── text_block_collector.rs   # TextBlockCollector（組み込みHandler）
│   └── tool_call_collector.rs    # ToolCallCollector（組み込みHandler）
└── event.rs             # Worker層の公開イベント型（timeline::eventからの変換含む）
```

イベント型は3層に存在する:

1. `llm_client::event` - プロバイダ正規化済みイベント
2. `timeline::event` - Timeline内部イベント（`llm_client::event`からの`From`変換）
3. `crate::event`（Worker層公開） - 外部公開イベント（`timeline::event`からの`From`変換）

## イベントモデル

前提：`llm_client`層は各プロバイダのストリーミングレスポンスを正規化し、**フラットなEvent列挙**として出力する。Timeline層はそれを受け取り同一構造の`timeline::event::Event`に変換する。

```rust
// timeline::event::Event
pub enum Event {
    // Meta events
    Ping(PingEvent),
    Usage(UsageEvent),
    Status(StatusEvent),
    Error(ErrorEvent),

    // Block lifecycle events
    BlockStart(BlockStart),
    BlockDelta(BlockDelta),
    BlockStop(BlockStop),
    BlockAbort(BlockAbort),
}
```

### メタイベント型

```rust
pub struct PingEvent {
    pub timestamp: Option<u64>,
}

pub struct UsageEvent {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

pub struct StatusEvent {
    pub status: ResponseStatus,
}

pub enum ResponseStatus {
    Started,
    Completed,
    Cancelled,
    Failed,
}

pub struct ErrorEvent {
    pub code: Option<String>,
    pub message: String,
}
```

### ブロックイベント型

```rust
pub enum BlockType {
    Text,
    Thinking,
    ToolUse,
    ToolResult,
}

pub struct BlockStart {
    pub index: usize,
    pub block_type: BlockType,
    pub metadata: BlockMetadata,
}

pub enum BlockMetadata {
    Text,
    Thinking,
    ToolUse { id: String, name: String },
    ToolResult { tool_use_id: String },
}

pub struct BlockDelta {
    pub index: usize,
    pub delta: DeltaContent,
}

pub enum DeltaContent {
    Text(String),
    Thinking(String),
    InputJson(String),
}

pub struct BlockStop {
    pub index: usize,
    pub block_type: BlockType,
    pub stop_reason: Option<StopReason>,
}

pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
}

pub struct BlockAbort {
    pub index: usize,
    pub block_type: BlockType,
    pub reason: String,
}
```

## TimelineとKind/Handler

Timelineは`Event`ストリームを受け取り、登録された`Handler<K: Kind>`にディスパッチする。

### Kind（`handler.rs`）

`Kind`はイベント型のみを定義する。スコープはHandler側で定義するため、同じKindに対して異なるスコープを持つHandlerを登録できる。

```rust
pub trait Kind {
    type Event;
}
```

### Handler（`handler.rs`）

`Handler`はKindに対する処理を定義し、自身のスコープ型も決定する。

```rust
pub trait Handler<K: Kind> {
    type Scope: Default;

    fn on_event(&mut self, scope: &mut Self::Scope, event: &K::Event);
}
```

- `Kind`によって受け取るイベント型が決定される
- `Handler::Scope`によってHandler固有のスコープ型が決定される
- Meta系とBlock系を統一的に扱える

### Meta系Kind

スコープ不要の単発イベント。登録時にスコープが即座に開始される:

```rust
pub struct UsageKind;
impl Kind for UsageKind {
    type Event = UsageEvent;
}

pub struct PingKind;
impl Kind for PingKind {
    type Event = PingEvent;
}

pub struct StatusKind;
impl Kind for StatusKind {
    type Event = StatusEvent;
}

pub struct ErrorKind;
impl Kind for ErrorKind {
    type Event = ErrorEvent;
}
```

### Block系Kind

ライフサイクル（Start/Delta/Stop）を持つ。スコープはHandler側で定義。Block系は3種定義されている:

#### TextBlockKind

```rust
pub struct TextBlockKind;
impl Kind for TextBlockKind {
    type Event = TextBlockEvent;
}

pub enum TextBlockEvent {
    Start(TextBlockStart),
    Delta(String),
    Stop(TextBlockStop),
}

pub struct TextBlockStart {
    pub index: usize,
}

pub struct TextBlockStop {
    pub index: usize,
    pub stop_reason: Option<StopReason>,
}
```

#### ThinkingBlockKind

```rust
pub struct ThinkingBlockKind;
impl Kind for ThinkingBlockKind {
    type Event = ThinkingBlockEvent;
}

pub enum ThinkingBlockEvent {
    Start(ThinkingBlockStart),
    Delta(String),
    Stop(ThinkingBlockStop),
}

pub struct ThinkingBlockStart {
    pub index: usize,
}

pub struct ThinkingBlockStop {
    pub index: usize,
}
```

#### ToolUseBlockKind

```rust
pub struct ToolUseBlockKind;
impl Kind for ToolUseBlockKind {
    type Event = ToolUseBlockEvent;
}

pub enum ToolUseBlockEvent {
    Start(ToolUseBlockStart),
    InputJsonDelta(String),
    Stop(ToolUseBlockStop),
}

pub struct ToolUseBlockStart {
    pub index: usize,
    pub id: String,
    pub name: String,
}

pub struct ToolUseBlockStop {
    pub index: usize,
    pub id: String,
    pub name: String,
}
```

### 使用例

```rust
// 使用例1: デルタを即時出力（スコープ不要）
struct PrintHandler;
impl Handler<TextBlockKind> for PrintHandler {
    type Scope = ();

    fn on_event(&mut self, _scope: &mut (), event: &TextBlockEvent) {
        if let TextBlockEvent::Delta(s) = event {
            print!("{}", s);
        }
    }
}

// 使用例2: テキストを蓄積して収集（Stringをスコープとして利用）
struct TextCollector { results: Vec<String> }
impl Handler<TextBlockKind> for TextCollector {
    type Scope = String;

    fn on_event(&mut self, buffer: &mut String, event: &TextBlockEvent) {
        match event {
            TextBlockEvent::Start(_) => {}
            TextBlockEvent::Delta(s) => buffer.push_str(s),
            TextBlockEvent::Stop(_) => {
                self.results.push(std::mem::take(buffer));
            }
        }
    }
}
```

## Timelineの責務

1. `Event`ストリームを受信
2. Meta系イベントを即座に該当Handlerへディスパッチ
3. Block系イベント（BlockStart/Delta/Stop/Abort）をBlockKindごとのライフサイクルイベントに変換
4. 各Handlerごとのスコープの生成・管理（BlockStart時に生成、BlockStop/Abort時に破棄）
5. 登録されたHandlerへの登録順ディスパッチ
6. 暗黙的なスコープ開始（BlockStartなしにDeltaが到達した場合の対応）

## Handlerの型消去（`timeline.rs`）

### Meta系: `ErasedHandler<K>`

各Handlerは独自のScope型を持つため、Timelineで保持するには型消去が必要。`Send + Sync`境界付き:

```rust
pub trait ErasedHandler<K: Kind>: Send + Sync {
    fn dispatch(&mut self, event: &K::Event);
    fn start_scope(&mut self);
    fn end_scope(&mut self);
}

pub struct HandlerWrapper<H, K>
where
    H: Handler<K>,
    K: Kind,
{
    handler: H,
    scope: Option<H::Scope>,
    _kind: PhantomData<fn() -> K>,  // Send+Sync安全なPhantomData
}
```

Meta系Handlerは登録時に`start_scope()`が呼ばれ、スコープが常にアクティブな状態で保持される。

### Block系: `ErasedBlockHandler`

Block系は`BlockStart`/`BlockDelta`/`BlockStop`/`BlockAbort`の4メソッドに分離されたtrait:

```rust
trait ErasedBlockHandler: Send + Sync {
    fn dispatch_start(&mut self, start: &BlockStart);
    fn dispatch_delta(&mut self, delta: &BlockDelta);
    fn dispatch_stop(&mut self, stop: &BlockStop);
    fn dispatch_abort(&mut self, abort: &BlockAbort);
    fn start_scope(&mut self);
    fn end_scope(&mut self);
    fn has_scope(&self) -> bool;
}
```

各BlockKindに対し専用のラッパー構造体が実装されている:

- `TextBlockHandlerWrapper<H>` - `DeltaContent::Text`のみをフィルタしてディスパッチ
- `ThinkingBlockHandlerWrapper<H>` - `DeltaContent::Thinking`のみをフィルタしてディスパッチ
- `ToolUseBlockHandlerWrapper<H>` - `DeltaContent::InputJson`をフィルタし、`BlockMetadata::ToolUse`からid/nameを追跡。Stopイベントにもid/nameを含めてディスパッチ

## Timeline構造体

```rust
pub struct Timeline {
    // Meta系ハンドラー
    usage_handlers: Vec<Box<dyn ErasedHandler<UsageKind>>>,
    ping_handlers: Vec<Box<dyn ErasedHandler<PingKind>>>,
    status_handlers: Vec<Box<dyn ErasedHandler<StatusKind>>>,
    error_handlers: Vec<Box<dyn ErasedHandler<ErrorKind>>>,

    // Block系ハンドラー（BlockTypeごとにグループ化）
    text_block_handlers: Vec<Box<dyn ErasedBlockHandler>>,
    thinking_block_handlers: Vec<Box<dyn ErasedBlockHandler>>,
    tool_use_block_handlers: Vec<Box<dyn ErasedBlockHandler>>,

    // 現在アクティブなブロック
    current_block: Option<BlockType>,
}
```

### ハンドラ登録メソッド

| メソッド | Kind | 備考 |
|---|---|---|
| `on_usage()` | `UsageKind` | Meta系（登録時にスコープ開始） |
| `on_ping()` | `PingKind` | Meta系 |
| `on_status()` | `StatusKind` | Meta系 |
| `on_error()` | `ErrorKind` | Meta系 |
| `on_text_block()` | `TextBlockKind` | Block系 |
| `on_thinking_block()` | `ThinkingBlockKind` | Block系 |
| `on_tool_use_block()` | `ToolUseBlockKind` | Block系 |

### ディスパッチ処理

```rust
impl Timeline {
    pub fn dispatch(&mut self, event: &Event) {
        match event {
            // Meta系: 即時ディスパッチ（登録順）
            Event::Usage(u) => self.dispatch_usage(u),
            Event::Ping(p) => self.dispatch_ping(p),
            Event::Status(s) => self.dispatch_status(s),
            Event::Error(e) => self.dispatch_error(e),

            // Block系: スコープ管理しながらディスパッチ
            Event::BlockStart(s) => self.handle_block_start(s),
            Event::BlockDelta(d) => self.handle_block_delta(d),
            Event::BlockStop(s) => self.handle_block_stop(s),
            Event::BlockAbort(a) => self.handle_block_abort(a),
        }
    }
}
```

Block系のディスパッチフロー:

- **BlockStart**: `current_block`を設定 → 該当BlockTypeのハンドラに対し`start_scope()` → `dispatch_start()`
- **BlockDelta**: `current_block`未設定時は暗黙的に設定。スコープ未開始のハンドラには暗黙的に`start_scope()`を呼ぶ → `dispatch_delta()`
- **BlockStop**: `dispatch_stop()` → `end_scope()` → `current_block`をクリア
- **BlockAbort**: `dispatch_abort()` → `end_scope()` → `current_block`をクリア

`BlockType::ToolResult`は`text_block_handlers`にルーティングされる（テキストとして扱う）。

### ブロック中断

```rust
pub fn abort_current_block(&mut self)
```

キャンセルやエラー時に呼び出し、進行中のブロックに対して`BlockAbort`イベントを発火してスコープをクリーンアップする。

## 組み込みHandler

### TextBlockCollector

テキストブロックを収集する組み込みHandler。`Arc<Mutex<Vec<String>>>`を内部に持ち、`Clone`可能。

```rust
pub struct TextBlockCollector {
    collected: Arc<Mutex<Vec<String>>>,
}

impl Handler<TextBlockKind> for TextBlockCollector {
    type Scope = TextCollectorState;  // buffer: String を保持
    // Start: バッファクリア
    // Delta: バッファに追記
    // Stop: バッファの内容をcollectedに確定
}
```

主なメソッド: `take_collected()`, `collected()`, `has_content()`, `clear()`

### ToolCallCollector

ToolUseブロックを収集する組み込みHandler。完了したツール呼び出しを`ToolCall`として収集する。

```rust
pub struct ToolCallCollector {
    collected: Arc<Mutex<Vec<ToolCall>>>,
}

impl Handler<ToolUseBlockKind> for ToolCallCollector {
    type Scope = CollectorState;  // current_id, current_name, input_json_buffer を保持
    // Start: id/nameを記録、バッファクリア
    // InputJsonDelta: JSONバッファに追記
    // Stop: バッファをパースしToolCallを確定
}
```

主なメソッド: `take_collected()`, `collected()`, `has_pending_calls()`, `clear()`

Workerは内部で`TextBlockCollector`と`ToolCallCollector`を自動的にTimelineに登録して使用する。

## 期待効果

- **統一インターフェース**: Meta系もBlock系も`Handler<K: Kind>`で統一
- **型安全**: `Kind`によってイベント型がコンパイル時に決定、`Handler::Scope`によってHandler固有のスコープ型が決定
- **スコープの柔軟性**: 同一Kindに対して異なるScopeを持つ複数Handlerを登録可能
- **責務分離**: `llm_client`層はフラットなEvent出力、Timeline層がブロック構造化、Worker層が公開イベントへの変換
- **スコープ管理の自動化**: Handlerは自前でスコープ保持を意識せずに済む
- **暗黙的スコープ開始**: BlockStartを送らないプロバイダ（OpenAI等）にも対応
- **Send + Sync**: 全ハンドラが`Send + Sync`境界を持ち、非同期コンテキストで安全に使用可能
