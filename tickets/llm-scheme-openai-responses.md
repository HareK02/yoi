# OpenAI Responses scheme の新設

> **レビュー完了（close 可）** — 詳細は [`llm-scheme-openai-responses.review.md`](llm-scheme-openai-responses.review.md)
> 9 要件すべて達成、指摘事項は優先度低の 2 件のみ（tool 引数 delta 先行時の空メタデータ、`tools[].strict` ハードコード）。いずれも実害薄。


## 背景

現状の `crates/llm-worker/src/llm_client/scheme/openai_chat` は OpenAI Chat Completions (`/v1/chat/completions`) wire format のみ実装。OpenAI の Responses API (`/v1/responses`) はリクエスト body・SSE イベント構造ともに Chat Completions と別物で、同じ scheme には乗らない。

Codex CLI (github.com/openai/codex) の実装を確認したところ、ChatGPT OAuth 経路でも OpenAI API Key 経路でもすべて `/v1/responses` を叩いており、Chat Completions は使っていない。Codex 流用（別チケット `llm-auth-codex-oauth`）を実現する前提として、この scheme が必要。

また OpenAI 本家の最新モデル（GPT-5 系・o シリーズの reasoning）は Responses API 経由が主要な経路であり、長期的にも Chat Completions の地位は低下していく。

## 要件

1. **`scheme/openai_responses` を新設**し、`HttpTransport<S: Scheme>` に差し込めるようにする

2. **リクエスト body** は `/v1/responses` の item-based 形式:
   - `model`, `instructions` (system prompt 相当), `input: [ResponseItem]`, `tools`
   - `tool_choice: "auto"` / `parallel_tool_calls: true` は **scheme 固定値**で常時送信（将来必要になれば Request / RequestConfig に昇格、今は YAGNI）
   - `reasoning: { effort?, summary? }` は `ReasoningControl` から投影
   - **`store: false` + `include: ["reasoning.encrypted_content"]` を scheme 固定値**で送信（stateless 運用 + 再送のため encrypted reasoning を取得）
   - `stream: true` 固定
   - `service_tier?`, `prompt_cache_key?`, `text?: { verbosity?, format? }` は当面未使用、フィールドの予約のみ
   - `previous_response_id` は **使わない**（stateless、履歴は insomnia 側管理）

3. **SSE event パース**:
   - `response.created` / `response.completed` / `response.failed` / `response.incomplete`
   - `response.output_item.added` / `response.output_item.done`
   - `response.content_part.added` / `response.content_part.done`
   - `response.output_text.delta`
   - `response.function_call_arguments.delta`（通常 function tool の引数 partial JSON）
   - `response.custom_tool_call_input.delta`（custom tool のフリーフォーム入力 partial JSON）
   - `response.reasoning_text.delta` / `response.reasoning_summary_text.delta`

4. **BlockType / DeltaContent との対応**:
   - **text BlockStart** は `response.content_part.added`（Anthropic の `content_block_start` と対称）
   - **tool_use BlockStart** は `response.output_item.added`（id と name が確定する時点、streaming に乗せるためここ）
   - `response.output_text.delta` → `DeltaContent::Text`
   - `response.reasoning_text.delta` / `response.reasoning_summary_text.delta` → `DeltaContent::Thinking`
   - `response.function_call_arguments.delta` と `response.custom_tool_call_input.delta` → 両方とも `DeltaContent::InputJson` に正規化
   - `response.content_part.done` / `response.output_item.done` → `BlockStop`

5. **`Item::Reasoning` の拡張**（llm-worker/types.rs への変更を含む）:

   ```rust
   Item::Reasoning {
       text: String,
       summary: Vec<String>,
       encrypted_content: Option<String>,
   }
   ```

   - 送信時は `input[]` の reasoning item に再構築（`encrypted_content` があれば添える）
   - 受信時は SSE から `text` / `summary[]` / `encrypted_content` を組み立てて `Item::Reasoning` に格納
   - 既存 `Item::Reasoning { text }` の 1 フィールドからの拡張。`summary` は空 Vec、`encrypted_content` は `None` で既存互換を保つ
   - 将来 Anthropic の extended thinking で `signature: Option<String>` を追加する余地を残す

6. **認証は `AuthRef::ApiKey` のみ対応**: `Authorization: Bearer <api_key>` ヘッダ。`base_url` デフォルトは `https://api.openai.com`、パスは `/v1/responses`。ChatGPT OAuth 経路（`CodexOAuth`）は別チケット（`llm-auth-codex-oauth`）で追加

7. **Usage の正規化**: `response.completed` の `usage: { input_tokens, output_tokens, total_tokens }` を `UsageEvent` に変換

8. **capability テーブル**: GPT-5 / o3 / o4 のモデル ID 判定は `scheme/openai_chat/capability.rs` と重複するため **共通関数に切り出して共有**（配置は `scheme/openai_chat/capability.rs` に `pub(crate) fn classify(model_id) -> Option<OpenAiFamily>` を置くか、`scheme/openai_common/` を切り出すかは実装時判断）。Responses 側は `ReasoningSupport::Effort` 固定でマッピング

9. **完了時の動作**: OpenAI API key (`OPENAI_API_KEY`) + モデル `gpt-5` 等で `ModelConfig { scheme: OpenAIResponses, base_url: https://api.openai.com, model_id: "gpt-5", auth: ApiKey }` を宣言すると、reasoning + tool call を含む会話が動作する

## 設計課題

### 1. scheme-specific 設定の override フィールド

`store` / `include[]` を scheme 固定値にしたが、将来 ZDR 非対応環境で `store=true` を許したくなる可能性がある。`OpenAIResponsesScheme` 自身にフィールド (`store: bool`, `include_encrypted_content: bool` 等) を持たせ、`new()` 時に上書きできる形にする。`ModelCapability` には入れない（scheme-specific な wire 設定なので）。

### 2. Responses 非対応パラメータ

`service_tier` / `prompt_cache_key` / `text.verbosity` は当面不要だが、将来対応時に scheme 拡張で入れられる構造にしておく。

## Scope 外

- ChatGPT OAuth 認証（`llm-auth-codex-oauth` チケットで実装）
- `previous_response_id` を使う stateful 運用
- 高次ツール（`web_search` / `code_interpreter` / `computer_use`）— insomnia では採用しない方針
- `tool_choice` / `parallel_tool_calls` の Request 昇格（必要性が出てから別チケット）

## 依存

- `tickets/llm-model-config.md` 完了済（`HttpTransport<S>` 構造と `AuthRef` が前提）

## 影響範囲

llm-worker 単独ではなく以下にまたがる:
- `crates/llm-worker/src/llm_client/types.rs`: `Item::Reasoning` の拡張
- `crates/llm-worker/src/llm_client/scheme/openai_responses/`: 新規
- `crates/llm-worker/src/llm_client/scheme/openai_chat/capability.rs`: モデル family 判定を `pub(crate)` に露出
- `crates/llm-worker/src/llm_client/scheme/mod.rs`: `pub mod openai_responses;`
- `crates/provider/src/lib.rs`: `build_client` の `SchemeKind::OpenaiResponses` アームを `SchemeNotImplemented` から実装に差し替え
