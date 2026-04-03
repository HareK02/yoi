# INSOMNIA

insomnia(i6a)は不休のエージェントループを回すためのエージェントプラットフォーム。

ワークフローを統括し、四六時中電力を消費し、イテレーションします。

## Crates

| クレート | 概要 |
|---|---|
| `insomnia` | トップレベルアプリケーション（未実装） |
| `llm-worker` | 自律的なLLMシステムを構築するためのライブラリ |
| `llm-worker-macros` | `llm-worker`用の手続きマクロ (`#[tool_registry]`, `#[tool]`) |

## ドキュメント

`llm-worker` の設計ドキュメントは [`crates/llm-worker/docs/`](crates/llm-worker/docs/) に配置されています。

### 設計仕様 (`spec/`)
- [basis](crates/llm-worker/docs/spec/basis.md) — 基盤用語とアーキテクチャ概要
- [timeline](crates/llm-worker/docs/spec/timeline.md) — Timeline抽象レイヤー設計
- [cache_lock](crates/llm-worker/docs/spec/cache_lock.md) — KVキャッシュ最適化（Type-stateパターン）
- [hooks](crates/llm-worker/docs/spec/hooks.md) — フックライフサイクルと介入システム
- [worker](crates/llm-worker/docs/spec/worker.md) — Workerオーケストレーション設計
- [cancellation](crates/llm-worker/docs/spec/cancellation.md) — 非同期キャンセル機構
- [tools](crates/llm-worker/docs/spec/tools.md) — ツールシステム設計

### 調査資料 (`research/`)
- [openresponses_mapping](crates/llm-worker/docs/research/openresponses_mapping.md) — Open Responsesマッピング
- [llm-streaming](crates/llm-worker/docs/research/2026-01-02-llm-streaming.md) — LLMストリーミング調査
- [provider-event-specs](crates/llm-worker/docs/research/2026-01-02-provider-event-specs.md) — プロバイダイベント仕様

### 計画 (`plan/`)
- [rig_adoption](crates/llm-worker/docs/plan/rig_adoption.md) — rig参照設計の採用計画
- [worker_api](crates/llm-worker/docs/plan/worker_api.md) — Worker API/DSL設計計画
