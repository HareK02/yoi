# llm-worker

LLMを用いた自律的なワーカーを構築するためのライブラリ。

ツールの定義・呼び出し、コンテキスト管理、ストリーミングイベント処理、フック制御などを責務にもつ。

ワークスペース `insomnia` に属し、以下の3クレートで構成される:

| クレート | 概要 |
|---|---|
| `insomnia` | アプリケーション本体 |
| `llm-worker` | LLMワーカーライブラリ (本クレート) |
| `llm-worker-macros` | `#[tool_registry]` / `#[tool]` 手続きマクロ |

## 用語定義

- **Item**:
  会話の基本単位。Open Responses仕様に準じた列挙型で、`Message`・`FunctionCall`・`FunctionCallOutput`・`Reasoning`の4バリアントを持つ。従来のメッセージベースAPIと異なり、ツール呼び出しや思考をファーストクラスのItemとして扱う。
- **ContentPart**:
  Message Item内のコンテンツ要素。`InputText`(ユーザー入力)・`OutputText`(アシスタント出力)・`Refusal`(拒否)を区別する。
- **ブロック (Block)**:
  モデルが「開始→デルタ→終了」で囲む一塊の出力。OpenAIの`response.output_item`やAnthropicの`content_block`、Geminiの`candidates[].content.parts`などが該当する。`BlockType`として`Text`・`Thinking`・`ToolUse`・`ToolResult`が定義されている。
- **メタイベント (Meta Event)**:
  `Ping`・`Usage`・`Status`・`Error`など、ブロックに属さない補助イベント群。ステータス更新やハートビートとして処理される。
- **Event**:
  LLMからのストリーミングレスポンスを表現する統一型。メタイベントとブロックイベント(`BlockStart`・`BlockDelta`・`BlockStop`・`BlockAbort`)から成る。`llm_client`層と`event`モジュールの双方に定義が存在する。
- **Worker**:
  LLMとのインタラクションを管理する中心コンポーネント。ツール実行ループ、フック呼び出し、ストリーミングイベントのディスパッチを統括する。

以降の仕様では「ブロック」を上記の生成・ツール単位の総称として扱う。

## アーキテクチャ

モジュール構成概念図:

```plaintext
llm-worker
├── worker          # Workerコア (実行ループ、状態管理)
├── state           # Type-stateパターン (Mutable / CacheLocked)
├── tool            # Tool trait, ToolMeta, ToolDefinition
├── tool_server     # ToolServer (ツールのレジストリと実行)
├── hook            # Hook trait, HookRegistry (10種のフックポイント)
├── handler         # Kind/Handler trait (イベントディスパッチ)
├── subscriber      # WorkerSubscriber trait (UI向けイベント購読)
├── event           # Worker層の公開Event型
├── message         # Item/ContentPart/Roleのre-export
├── timeline        # イベントストリームの状態管理とHandlerディスパッチ
│   ├── event       # Timeline内部イベント型
│   ├── timeline    # Timelineコア
│   ├── text_block_collector   # テキスト収集Handler
│   └── tool_call_collector    # ツール呼び出し収集Handler
└── llm_client      # LLMクライアント層
    ├── client      # LlmClient trait
    ├── event       # llm_client層のEvent型
    ├── error       # ClientError
    ├── types       # Item, ContentPart, Role, Request, RequestConfig, ToolDefinition
    ├── scheme      # APIスキーマ変換
    │   ├── openai       # OpenAI Chat Completions API
    │   ├── anthropic    # Anthropic Messages API
    │   └── gemini       # Google Gemini API
    └── providers   # プロバイダ固有クライアント
        ├── openai       # OpenAI
        ├── anthropic    # Anthropic
        ├── gemini       # Google Gemini
        └── ollama       # Ollama (ローカルLLM)
```

OpenAI互換のプロバイダでスキーマを使い回せるよう、`scheme`と`providers`モジュールは分離されている。

### scheme層

単純な変換を責務とするスキーマを定義する。

- リクエスト変換: `Request`(SystemPrompt + Items + Tools + RequestConfig) → プロバイダ固有のリクエストJSON
- レスポンス変換: SSEイベント → 型付き`Event`構造体のストリーム

各APIスキーマ(OpenAI / Anthropic / Gemini)ごとに実装を持ち、APIスキーマに準じたパース・バリデーションを行う。

### llm_client (providers) 層

`LlmClient` traitを実装する各プロバイダクライアントがリクエストを送信し、差異が吸収され統一された`Event`ストリームを出力する。

ストリーミング中のバッファリング(ToolArguments の累積等)もこの層で行う。

`ConfigWarning`により、プロバイダがサポートしない設定オプションを警告として通知する仕組みを持つ。

### timeline層

`llm_client`からのイベントストリームを受信し、登録された`Handler`にディスパッチする。

`Kind` traitがイベント型を定義し、`Handler<K: Kind>` traitが`Scope`(ブロックのライフサイクルに対応する状態)を持つイベント処理を行う。組み込みHandlerとして`TextBlockCollector`と`ToolCallCollector`が提供される。

### worker層

`Worker`はType-stateパターンにより`Mutable`状態と`CacheLocked`状態を持つ。

- **Mutable**: システムプロンプトの設定変更、メッセージ履歴の編集、ツール・フックの登録が可能
- **CacheLocked**: `Worker::lock()`で遷移。LLM APIのKVキャッシュヒット率を最大化するため、既存履歴の変更を制限する

`WorkerSubscriber` traitにより、テキスト生成やツール呼び出しのストリーミングイベントをUI等に配信できる。

## Tools

各種プロバイダのLLMのAPI仕様として存在するTool CallやFunction Callingを`Tool` traitとして統一的に扱う。

- **`ToolMeta`**: ツール名・説明・入力スキーマ(JSON Schema)を保持する不変のメタ情報。Worker登録後は変更されない。
- **`ToolDefinition`**: `Fn() -> (ToolMeta, Arc<dyn Tool>)`型のファクトリ。Worker登録時に一度呼び出され、メタ情報とインスタンスがセッションスコープでキャッシュされる。
- **`Tool` trait**: `async fn call(&self, arguments: Value) -> Result<String, ToolError>`を持つ非同期trait。セッション中に状態を保持できる。
- **`ToolServer`**: 登録済みツールのレジストリと実行を管理するインメモリサーバー。

`llm-worker-macros`クレートが提供する`#[tool_registry]` / `#[tool]`手続きマクロにより、`impl`ブロック上のメソッドから`Tool` trait実装を自動生成する。

## Hooks

Claude Codeに存在するようなHooksと同様のターン制御・介入機構を搭載する。

以下の10種のフックポイントが定義されている:

| フックポイント | タイミング | 戻り値による制御 |
|---|---|---|
| `OnPromptSubmit` | プロンプト送信時 | Continue / Cancel |
| `PreLlmRequest` | LLMリクエスト直前 | Continue / Cancel |
| `PreToolCall` | ツール実行直前 | Continue / Skip / Abort / Pause |
| `PostToolCall` | ツール実行直後 | Continue / Abort |
| `OnTurnEnd` | ターン終了時 | Finish / ContinueWithMessages / Paused |
| `OnAbort` | 中断時 | (通知のみ) |
| `OnTextDelta` | テキストデルタ受信時 | (ストリーミング) |
| `OnToolCallDelta` | ツール引数デルタ受信時 | (ストリーミング) |
| `OnStreamChunk` | ストリームチャンク受信時 | (ストリーミング) |
| `OnStreamComplete` | ストリーム完了時 | (ストリーミング) |

フックは`Hook` traitとして表現され、`HookRegistry`で管理される。

## ストリーミングイベント処理

イベントストリームを扱う設計目標として、Text / InputJSON / Thinkingのデルタと、[細粒度ツールストリーミング](https://platform.claude.com/docs/en/agents-and-tools/tool-use/fine-grained-tool-streaming)を含む、デルタの低遅延・リアルタイム処理がある。

ブロックのライフサイクル: `BlockStart` → `BlockDelta`(複数回) → `BlockStop` / `BlockAbort`

`DeltaContent`の種別:
- `Text(String)` - テキスト生成のデルタ
- `Thinking(String)` - 思考(Extended Thinking等)のデルタ
- `InputJson(String)` - ツール引数JSONの部分文字列デルタ
