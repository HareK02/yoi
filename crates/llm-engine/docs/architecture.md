# llm-engine アーキテクチャ

## 概要

llm-engineは3層構成でLLMとのインタラクションを管理する。

```
┌─────────────────────────────────────────┐
│  Engine (オーケストレーション)            │
│  ターンループ / フック / ツール実行       │
│  Type-state: Mutable ↔ CacheLocked      │
└───────────┬─────────────────────────────┘
            │
┌───────────▼─────────────────────────────┐
│  Timeline (イベント処理)                 │
│  Handler dispatch / Block collectors     │
└───────────┬─────────────────────────────┘
            │
┌───────────▼─────────────────────────────┐
│  LLM Client (プロトコル)                 │
│  Provider (HTTP) / Scheme (変換)         │
│  Anthropic / OpenAI / Gemini / Ollama    │
└─────────────────────────────────────────┘
```

## モジュール構成

| モジュール | 責務 | 要件 |
|---|---|---|
| `engine` | ターンループ、フック統合、ツール実行、Pause/Resume | R1, R4 |
| `state` | Type-state (Mutable/CacheLocked) | R2 |
| `hook` | Hook trait、10フックポイント | R3, R4 |
| `tool` / `tool_server` | ツール定義・登録・実行 | R3 |
| `timeline` | イベントストリーム処理、Handler dispatch | — |
| `handler` | Handler/Kind trait、ブロック別ハンドラ | — |
| `callback` | クロージャベースイベント購読（`on_text_block`, `on_usage` 等） | — |
| `llm_client` | LLMプロバイダへのHTTPリクエスト/ストリーミング | — |
| `llm_client/scheme` | プロバイダ固有ワイヤーフォーマット変換 | — |
| `llm_client/providers` | Anthropic, OpenAI, Gemini, Ollama実装 | — |

## データフロー

### リクエスト（送信）
```
Engine.history (Vec<Item>)
  → build_request() → Request { items, tools, config }
    → Scheme.build_request() → プロバイダ固有JSON
      → Provider.stream() → HTTP POST
```

### レスポンス（受信）
```
HTTP SSE bytes
  → Provider → SSE events
    → Scheme.parse_event() → Event (統一型)
      → Timeline.dispatch() → Handler.on_event()
        → TextBlockCollector / ToolCallCollector
          → Engine: 履歴に追加、ツール実行判定
```

## 内部型

### Item (会話履歴の単位)
- `Item::Message` — テキストメッセージ (user/assistant)
- `Item::ToolCall` — ツール呼び出し
- `Item::ToolResult` — ツール実行結果
- `Item::Reasoning` — 思考 (Extended Thinking)

### Event (ストリーミングイベント)
- Meta: `Ping`, `Usage`, `Status`, `Error`
- Block: `BlockStart` → `BlockDelta`* → `BlockStop` / `BlockAbort`

単一の `Event` 型が全層で共有される（`llm_client::event` で定義、他層はre-export）。
