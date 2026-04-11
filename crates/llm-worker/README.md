# llm-worker

LLM との対話を管理する低レベル基盤クレート。会話履歴、ツール実行、イベントストリーミング、ライフサイクルフックを統合した `Worker` 抽象を提供する。

## 公開型

### コア

- `Worker<C, S>` — LLM 対話の中央管理（ターン実行、ツール呼び出し、キャンセル）
- `WorkerConfig` / `WorkerResult` / `WorkerError` — 設定・実行結果・エラー
- `Item` / `ContentPart` / `Role` — 会話履歴の構成要素

### モジュール

- `llm_client` — プロバイダ抽象（`LlmClient` トレイト、`Request`, `RequestConfig`, Anthropic/OpenAI/Gemini/Ollama 実装）
- `tool` — ツール定義・実行（`Tool` トレイト、`ToolDefinition`, `ToolOutput`, サイズ判定による Inline/Stored 切替）
- `tool_server` — ツール登録・ルックアップ（`ToolServer`, `ToolServerHandle`）
- `hook` — 実行フローへの介入ポイント（`Hook` トレイト、`PreToolCall`, `PostToolCall`, `OnTurnEnd` など）
- クロージャベースイベント購読（`Worker::on_text_block()`, `on_tool_use_block()`, `on_usage()` 等）
- `timeline` — イベントストリームのディスパッチ（`Handler` トレイト、各ブロックコレクター）。パワーユーザー向けに `timeline_mut()` も提供
- `event` — ストリーミングイベント型（`Event`, `BlockStart`, `BlockDelta` など）
- `state` — 型状態パターンによるキャッシュ保護（`Mutable` / `CacheLocked`）
cratesの整理Add READMEsRE to all crates@@
