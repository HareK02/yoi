# Tool 設計

## 概要

`llm-worker`のツールシステムは、LLMが外部リソースにアクセスしたり計算を実行するための仕組みを提供する。
メタ情報の不変性とセッションスコープの状態管理を両立させる設計となっている。

ツールの管理は`ToolServer` / `ToolServerHandle`に分離されており、
Workerから独立したツール管理・実行が可能。

## 主要な型

```
type ToolDefinition
  Fn() -> (ToolMeta, Arc<dyn Tool>)

Worker.register_tool() → ToolServerHandle.register_tool() で呼び出し

           |
           v

- struct ToolMeta (name, desc, schema)
    不変・登録時固定
- trait Tool (execute)
    登録時生成・セッション中再利用
- struct ToolServer / ToolServerHandle
    ツールの管理・実行を担当
```

### ToolMeta

ツールのメタ情報を保持する不変構造体。登録時に固定され、Worker内で変更されない。

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolMeta {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl ToolMeta {
    pub fn new(name: impl Into<String>) -> Self;
    pub fn description(mut self, desc: impl Into<String>) -> Self;
    pub fn input_schema(mut self, schema: Value) -> Self;
}
```

**目的:**

- LLM へのツール定義として送信
- Hook からの参照（読み取り専用）
- 登録後の不変性を保証

### Tool trait

ツールの実行ロジックのみを定義するトレイト。

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    async fn execute(&self, input_json: &str) -> Result<String, ToolError>;
}
```

**設計方針:**

- メタ情報（name, description, schema）は含まない
- 状態を持つことが可能（セッション中のカウンターなど）
- `Send + Sync` で並列実行に対応

**インスタンスのライフサイクル:**

1. `register_tool()` 呼び出し時にファクトリが実行され、インスタンスが生成される
2. LLM がツールを呼び出すと、既存インスタンスの `execute()` が実行される
3. 同じセッション中は同一インスタンスが再利用される

※ 「最初に呼ばれたとき」の遅延初期化ではなく、**登録時の即時初期化**である。

### ToolDefinition

メタ情報とツールインスタンスを生成するファクトリ。

```rust
pub type ToolDefinition = Arc<dyn Fn() -> (ToolMeta, Arc<dyn Tool>) + Send + Sync>;
```

**なぜファクトリか:**

- Worker への登録時に一度だけ呼び出される
- メタ情報とインスタンスを同時に生成し、整合性を保証
- クロージャでコンテキスト（`self.clone()`）をキャプチャ可能

### ToolError

```rust
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Internal error: {0}")]
    Internal(String),
}
```

## ToolServer / ToolServerHandle

ツール管理がWorkerから分離された専用コンポーネント。
`ToolServer`がツールマップを所有し、`ToolServerHandle`がそのクローン可能な参照を提供する。

### ToolServer

```rust
#[derive(Clone, Default)]
pub struct ToolServer {
    tools: Arc<Mutex<ToolMap>>,  // HashMap<String, (ToolMeta, Arc<dyn Tool>)>
}

impl ToolServer {
    pub fn new() -> Self;
    pub fn handle(&self) -> ToolServerHandle;
}
```

### ToolServerHandle

```rust
#[derive(Clone, Default)]
pub struct ToolServerHandle {
    tools: Arc<Mutex<ToolMap>>,
}

impl ToolServerHandle {
    // 登録（pub(crate) - Worker経由でのみ呼び出し可能）
    pub(crate) fn register_tool(&self, factory: WorkerToolDefinition) -> Result<(), ToolServerError>;
    pub(crate) fn register_tools(&self, factories: impl IntoIterator<Item = WorkerToolDefinition>) -> Result<(), ToolServerError>;

    // 検索（Hookからのアクセス用）
    pub fn get_tool(&self, name: &str) -> Option<(ToolMeta, Arc<dyn Tool>)>;

    // 実行
    pub async fn call_tool(&self, name: &str, input_json: &str) -> Result<String, ToolServerError>;

    // LLMリクエスト用定義生成（名前順ソート済み）
    pub fn tool_definitions_sorted(&self) -> Vec<LlmToolDefinition>;
}
```

### ToolServerError

```rust
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ToolServerError {
    #[error("Tool with name '{0}' already registered")]
    DuplicateName(String),
    #[error("Tool '{0}' not found")]
    ToolNotFound(String),
    #[error("Tool execution failed: {0}")]
    ToolExecution(String),
}
```

### 設計上のポイント

- `ToolServer`は`Clone`可能で、`Arc<Mutex<ToolMap>>`により複数箇所で共有可能
- `register_tool` / `register_tools` は `pub(crate)` でWorker経由のみ
- `get_tool`, `call_tool` は `pub` でHookやアプリケーションから直接利用可能
- `tool_definitions_sorted()` は名前でソートされた決定的な定義リストを返す
- Worker側では `tool_server_handle()` で `ToolServerHandle` を取得可能

## Worker でのツール管理

Worker は `ToolServerHandle` を内部に持ち、ツール登録のAPIを提供する。
登録は `Mutable` 状態でのみ可能。

```rust
// Worker API (Mutable状態のみ)
pub fn register_tool(&mut self, factory: WorkerToolDefinition) -> Result<(), ToolRegistryError>;
pub fn register_tools(&mut self, factories: impl IntoIterator<Item = WorkerToolDefinition>) -> Result<(), ToolRegistryError>;

// ハンドルの取得（全状態）
pub fn tool_server_handle(&self) -> ToolServerHandle;
```

登録時の処理:

1. ファクトリを呼び出し `(meta, instance)` を取得
2. 同名ツールが既に登録されていればエラー（`ToolRegistryError::DuplicateName`）
3. HashMap に `(meta, instance)` を保存

## マクロによる自動生成 (`llm-worker-macros`)

`#[tool_registry]` マクロと `#[tool]` マクロにより、メソッドから自動的に
`Tool` トレイト実装と `ToolDefinition` ファクトリを生成する。

### マクロ一覧

| マクロ | 種類 | 用途 |
| --- | --- | --- |
| `#[tool_registry]` | proc_macro_attribute | `impl`ブロックに適用。`#[tool]`メソッドを検出し、コード生成 |
| `#[tool]` | proc_macro_attribute | メソッドに適用。マーカー属性（`tool_registry`が処理） |
| `#[description]` | proc_macro_attribute | 引数に適用。schemarsの`description`に変換 |

### 使用例

```rust
#[tool_registry]
impl MyApp {
    /// 検索を実行する
    /// 指定されたクエリでデータベースを検索します。
    #[tool]
    async fn search(
        &self,
        #[description = "検索クエリ文字列"] query: String,
    ) -> String {
        format!("Results for: {}", query)
    }
}

// 生成されるもの:
// - SearchArgs 構造体
// - ToolSearch 構造体 (Tool trait実装)
// - MyApp::search_definition() メソッド
```

### 生成されるコード詳細

マクロは以下を自動生成します。

**1. 引数構造体（`{PascalCase}Args`）**

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    #[schemars(description = "検索クエリ文字列")]
    pub query: String,
}
```

- `#[description = "..."]` 属性は `#[schemars(description = "...")]` に変換される
- 引数がない場合は空の構造体が生成される

**2. ラッパー構造体（`Tool{PascalCase}`）**

```rust
#[derive(Clone)]
pub struct ToolSearch {
    ctx: MyApp,
}

#[async_trait]
impl Tool for ToolSearch {
    async fn execute(&self, input_json: &str) -> Result<String, ToolError> {
        let args: SearchArgs = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(e.to_string()))?;
        let result = self.ctx.search(args.query).await;
        Ok(format!("{:?}", result))
    }
}
```

- 戻り値が `Result` の場合、`Err` は `ToolError::ExecutionFailed` に変換される
- 戻り値が `Result` でない場合、`format!("{:?}", result)` で文字列化される
- async/非asyncの両方をサポート

**3. ファクトリメソッド（`{method_name}_definition`）**

```rust
impl MyApp {
    pub fn search_definition(&self) -> ToolDefinition {
        let ctx = self.clone();
        Arc::new(move || {
            let schema = schemars::schema_for!(SearchArgs);
            let meta = ToolMeta::new("search")
                .description("検索を実行する\n指定されたクエリでデータベースを検索します。")
                .input_schema(serde_json::to_value(schema).unwrap_or(json!({})));
            let tool: Arc<dyn Tool> = Arc::new(ToolSearch { ctx: ctx.clone() });
            (meta, tool)
        })
    }
}
```

- ツール名はメソッド名（snake_case）がそのまま使われる
- ツールの説明はメソッドのdocコメント全体が使われる（docコメントがない場合は `"Tool: {name}"` がフォールバック）
- `input_schema` は `schemars::schema_for!` で生成される

### 命名規則

| 元のメソッド名 | 引数構造体 | ラッパー構造体 | ファクトリメソッド |
| --- | --- | --- | --- |
| `search` | `SearchArgs` | `ToolSearch` | `search_definition()` |
| `get_user` | `GetUserArgs` | `ToolGetUser` | `get_user_definition()` |

PascalCaseへの変換は `to_pascal_case()` 関数で行われる（snake_caseの各セグメントを先頭大文字に変換）。

## Hook との連携

Hook は `ToolCallContext` / `PostToolCallContext`
を通じてメタ情報とインスタンスにアクセスできる。

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

**用途:**

- `meta` で名前やスキーマを確認
- `tool` でツールの内部状態を読み取り（ダウンキャスト必要）
- `call` の引数を改変してツールに渡す（PreToolCall）
- `result` の内容を改変（PostToolCall）

## 使用例

### 手動実装

```rust
struct Counter { count: AtomicUsize }

#[async_trait]
impl Tool for Counter {
    async fn execute(&self, _: &str) -> Result<String, ToolError> {
        let n = self.count.fetch_add(1, Ordering::SeqCst);
        Ok(format!("count: {}", n))
    }
}

let def: ToolDefinition = Arc::new(|| {
    let meta = ToolMeta::new("counter")
        .description("カウンターを増加")
        .input_schema(json!({"type": "object"}));
    (meta, Arc::new(Counter { count: AtomicUsize::new(0) }))
});

worker.register_tool(def)?;
```

### マクロ使用（推奨）

```rust
#[derive(Clone)]
struct App;

#[tool_registry]
impl App {
    /// 挨拶する
    #[tool]
    async fn greet(&self, #[description = "名前"] name: String) -> String {
        format!("Hello, {}!", name)
    }
}

let app = App;
worker.register_tool(app.greet_definition())?;
```

### 複数ツールの一括登録

```rust
worker.register_tools([
    app.search_definition(),
    app.get_user_definition(),
    app.greet_definition(),
])?;
```

## 設計上の決定

| 問題 | 決定 | 理由 |
| --- | --- | --- |
| メタ情報の変更可能性 | ToolMeta を分離・不変化 | 登録後の整合性を保証 |
| 状態管理 | 登録時にインスタンス生成 | セッション中の状態保持、同一インスタンス再利用 |
| Factory vs Instance | Factory + 登録時即時呼び出し | コンテキストキャプチャと登録時検証 |
| Hook からのアクセス | Context に meta と tool を含む | 柔軟な介入を可能に |
| ツール管理の分離 | ToolServer / ToolServerHandle | Worker外からのツール検索・実行を可能に |
| 登録の可視性 | `register_tool` は `pub(crate)` | Worker経由でのみ登録を許可し、整合性を維持 |
| 定義リストの順序 | 名前順ソート | 決定的なリクエスト生成を保証 |
