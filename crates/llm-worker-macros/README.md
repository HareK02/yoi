# llm-worker-macros

Rust メソッドを LLM 呼び出し可能なツールとして自動登録する手続きマクロクレート。引数構造体・Tool トレイト実装・ToolDefinition を自動生成する。

## 公開マクロ

- `#[tool_registry]` — impl ブロックに付与し、内部の `#[tool]` メソッドを一括処理
- `#[tool]` — メソッドをツールとしてマーク
- `#[description = "..."]` — 引数に説明を付与（JSON Schema の description に反映）
