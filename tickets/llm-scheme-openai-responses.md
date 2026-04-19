# OpenAI Responses scheme の新設

## 背景

現状の `crates/llm-worker/src/llm_client/scheme/openai` は OpenAI Chat Completions (`/v1/chat/completions`) wire format のみ実装。OpenAI の Responses API (`/v1/responses`) はリクエスト body・SSE イベント構造ともに Chat Completions と別物で、同じ scheme には乗らない。

Codex CLI (github.com/openai/codex) の実装を確認したところ、ChatGPT OAuth 経路でも OpenAI API Key 経路でもすべて `/v1/responses` を叩いており、Chat Completions は使っていない。Codex 流用（別チケット `llm-auth-codex-oauth`）を実現する前提として、この scheme が必要。

また OpenAI 本家の最新モデル（GPT-5 系・o シリーズの reasoning）は Responses API 経由が主要な経路であり、長期的にも Chat Completions の地位は低下していく。

## 要件

1. **`scheme/openai_responses` を新設**し、`HttpTransport<S: Scheme>` に差し込めるようにする

2. **リクエスト body** は `/v1/responses` の item-based 形式:
   - `model`, `instructions` (system prompt 相当), `input: [ResponseItem]`, `tools`, `tool_choice`, `parallel_tool_calls`
   - `reasoning: { effort?, summary? }`
   - `store`, `stream: true`, `include: [String]`
   - `service_tier?`, `prompt_cache_key?`, `text?: { verbosity?, format? }`
   - `previous_response_id` は **使わない**（stateless で運用、履歴は insomnia 側管理）

3. **SSE event パース**:
   - `response.created` / `response.completed` / `response.failed` / `response.incomplete`
   - `response.output_item.added` / `response.output_item.done`
   - `response.output_text.delta`
   - `response.custom_tool_call_input.delta`（ToolCall 引数の partial JSON）
   - `response.reasoning_text.delta` / `response.reasoning_summary_text.delta`

4. **BlockType / DeltaContent との対応**:
   - `response.output_text.delta` → `DeltaContent::Text`
   - `response.reasoning_text.delta` / `response.reasoning_summary_text.delta` → `DeltaContent::Thinking`
   - `response.custom_tool_call_input.delta` → `DeltaContent::InputJson`
   - `response.output_item.done` (tool_use) → `BlockMetadata::ToolUse { id, name }` の `BlockStart` 生成

5. **reasoning の item 構造対応**: `summary[]` / `encrypted_content` を持つ reasoning item の送受信をロスなく扱える
   - 送信時は `BlockMetadata::Thinking` から `input[]` に再構築
   - 受信時は `BlockType::Thinking` のブロックとしてストリームに流す

6. **認証は `AuthRef::ApiKey` のみ対応**: `Authorization: Bearer <api_key>` ヘッダ。`base_url` デフォルトは `https://api.openai.com`、パスは `/v1/responses`。ChatGPT OAuth 経路（`CodexOAuth`）は別チケット（`llm-auth-codex-oauth`）で追加

7. **Usage の正規化**: `response.completed` の `usage: { input_tokens, output_tokens, total_tokens }` を `UsageEvent` に変換。Chat Completions の `prompt_tokens` 等との表記揺れを scheme 側で吸収

8. **完了時の動作**: OpenAI API key (`OPENAI_API_KEY`) + モデル `gpt-5` 等で `ModelConfig { scheme: OpenAIResponses, base_url: https://api.openai.com, model_id: "gpt-5", auth: ApiKey }` を宣言すると、reasoning + tool call を含む会話が動作する

## 設計課題

### 1. reasoning item の encrypted_content

reasoning item の `encrypted_content` はサーバ側で暗号化された状態で返されることがあり、再送時にそのまま添える必要がある（ZDR 組織や `store=false` 運用時）。insomnia の `Item` enum に透過的に保持する仕組みが要る。

### 2. `include[]` と `store` のデフォルト

- `include: ["reasoning.encrypted_content"]` を常に付けるか、capability / config で制御するか
- `store=false` をデフォルトにするか `true` にするか（ZDR 既定なら false）

### 3. Responses 非対応パラメータ

`service_tier` / `prompt_cache_key` / `text.verbosity` は当面不要かもしれないが、将来対応時に scheme 拡張で入れられる構造にしておく。

## Scope 外

- ChatGPT OAuth 認証（`llm-auth-codex-oauth`）
- `previous_response_id` を使う stateful 運用
- 高次ツール（`web_search` / `code_interpreter` / `computer_use`）— insomnia では採用しない方針

## 依存

- `tickets/llm-model-config.md`（`HttpTransport<S>` 構造と `AuthRef` が前提）
