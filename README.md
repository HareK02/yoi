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

- [要件](crates/llm-worker/docs/requirements.md) — llm-workerに求める性能 (R1-R4)
- [アーキテクチャ](crates/llm-worker/docs/architecture.md) — 3層構成とモジュール配置
