# LLM モデル設定の再編

> **レビュー中** — 詳細は [`llm-model-config.review.md`](llm-model-config.review.md)
> 主な指摘: 要件 6 の `ModelConfig.capability` override 未実装、`validate_config` の機能退化（OpenAI top_k warning 消失）。どちらも判断待ち。


## 背景

決定済みの LLM プロバイダサポート方針（`docs/plan/llm_providers.md`）に従って llm-worker のプロバイダ層を再編する。Pod 側で「使う LLM モデル」を宣言する構造にし、共通の通信層 + scheme の組合せで任意のプロバイダを収容できるようにする。

### 現状の問題

- `crates/llm-worker/src/llm_client/providers/{anthropic,openai,gemini,ollama}.rs` は各々 HTTP 骨格 + scheme 呼び出しの薄いアダプタで、構造が重複
- `providers/ollama.rs` (67行) は scheme 未分離で整合性が崩れている
- OpenAI 互換ルーター系（xAI / Groq / OpenRouter / Together / BLACKBOX 等）を追加するたびに新ファイルを書く必要がある

## 要件

1. **Pod マニフェストで LLM モデルを宣言できる**

   ```
   ModelConfig {
       scheme: Scheme,      // Anthropic / OpenAIChat / OpenAIResponses / Gemini
       base_url: Url,
       model_id: String,
       auth: AuthRef,
   }
   ```

   継承（commit ebee0b9）と整合し、親 Pod の定義を子 Pod が override 可能。

2. **`providers/` 層の廃止**: llm-worker は `HttpTransport<S: Scheme>` 相当の汎用通信層 1 本を持ち、`ModelConfig` を食わせてインスタンス化する

3. **既存 scheme の再編**:
   - `scheme/openai` を `scheme/openai_chat` にリネーム
   - `scheme/anthropic` / `scheme/gemini` はそのまま
   - `scheme/openai_responses` は別チケット（llm-scheme-openai-responses）で新設
   - Ollama は **scheme/anthropic を base_url 差し替えで流用**（独自 scheme は作らない）

4. **認証の分離**: `AuthRef` は `ApiKey(EnvVar | ConfigRef)` / `CodexOAuth` / `None` を表現でき、scheme とは直交する層で管理される。`CodexOAuth` の実装自体は別チケット（llm-auth-codex-oauth）

5. **決定済みプロバイダ方針との整合**:
   - 第一級: Ollama / Codex OAuth / Anthropic API
   - 二次: OpenAI 互換共通枠（`{ scheme: OpenAIChat, base_url: 各社, auth: ApiKey }` の宣言だけで収容）

6. **ModelCapability を分離**: モデルに紐づく機能差を別メタデータとして表現

   ```
   ModelCapability {
       tool_calling: ToolCallingSupport,        // parallel 可否含む
       structured_output: StructuredOutput,     // JsonObject / JsonSchema
       reasoning: Option<ReasoningSupport>,     // effort / budget_tokens
       vision: bool,
       prompt_caching: CacheStrategy,           // Explicit { max_breakpoints } / Auto
   }
   ```

   `ReasoningControl { effort, budget_tokens }` は共通型、scheme アダプタで各社形式に投影。プロバイダ側高次ツール（web_search 等）は採用しないため軸から除外。

7. **Streaming は現状維持**: 既存 `BlockStart / BlockDelta / BlockStop / BlockAbort` + `DeltaContent::{Text, Thinking, InputJson}` を変更しない。Gemini や Ollama のように ToolCall 引数 delta を送らないプロバイダは scheme アダプタで「BlockStart → InputJson(全体 1 回) → BlockStop」の擬似ストリーム化で吸収

8. **Ollama 運用の注意点**: scheme/anthropic 流用前提で以下を守る
   - `cache_control` は送らない（`ModelCapability::prompt_caching = Auto`）
   - `tool_choice` / `metadata` / URL 画像は送らない（Ollama 側非対応）
   - `/v1/messages/count_tokens` は叩かない（issue #13949 でサーバ不安定化）
   - `/v1/chat/completions` は stream+tools バグ (#9092) のため使わない

9. **完了時の動作**: 既存の動作は変わらず、Pod マニフェストで `ModelConfig` を宣言するだけでモデル切替できる。OpenAI 互換の新規プロバイダは新コードなしで追加可能

## 設計決定

### 1. `Scheme` trait の境界（方針A: 全面抽象化）

trait で URL 組立・認証要件・body 変換・SSE パースをすべて抽象化し、`HttpTransport` は 1 本にする。trait スケッチ:

```rust
trait Scheme {
    fn path(&self, model: &str) -> String;               // "/v1/messages" 等
    fn required_auth(&self) -> AuthRequirement;          // Bearer / XApiKey / QueryParam / None
    fn additional_headers(&self) -> Vec<(&str, String)>; // anthropic-version 等
    fn build_request_body(&self, model: &str, req: &Request, cap: &ModelCapability) -> Value;
    fn parse_sse(&self, event_type: &str, data: &str) -> Result<Vec<Event>, ClientError>;
    fn default_base_url(&self) -> &'static str;
}
```

`parse_sse` は `Vec<Event>` に統一（Anthropic は 1 要素 Vec で扱う）。

### 2. `AuthRef` と `Scheme` の組合せ検証（方針B: 構築時検証）

`Scheme::required_auth()` で要求する `AuthRequirement` を宣言し、`build_client` 時に `AuthRef` と照合。非対応組合せは構築エラーにする（実行時に落とさない）。

### 3. `crates/provider/` の去就（方針A: 残す）

`provider` クレートは保持。`build_client(ModelConfig) -> Box<dyn LlmClient>` は `(Scheme, AuthRef)` 照合 + `HttpTransport::new` の薄ラッパーに縮退する。`~/.codex/auth.json` 読み取り等の認証ストア解決は `provider` クレートに肉付けしていく（llm-worker は低レベル基盤に留める方針と整合）。

### 4. `ModelCapability` の保持（方針: ハイブリッド）

- scheme 実装側に `model_id → ModelCapability` の静的テーブルを持つ（既知モデル分）
- `ModelConfig` で明示宣言すれば override
- 未知モデルは scheme ごとの安全側デフォルト（`prompt_caching: Auto` 等）にフォールバック

今チケットのスコープ:
- 型定義 + 既知モデルの固定値
- `CacheStrategy::Explicit` で `cache_control` マーカー挿入（既存実装を capability で分岐）
- `ReasoningControl { effort, budget_tokens }` を scheme 側で各社形式に投影
- `CacheStrategy::Auto` は何もしない（Ollama で重要）

### 5. マニフェスト継承との統合（方針: フィールド単位 override）

`ProviderConfigPartial` と同じ方針で、`ModelConfig` の全フィールド（`scheme` / `base_url` / `model_id` / `auth`）を `Option<T>` で継承。子 Pod は `base_url` だけ差し替える等が可能。

### 6. TOML 後方互換

旧 `[provider] kind = "..."` フォーマットは互換を切る。新 `[model]` セクションで `scheme` / `base_url` / `model_id` / `auth` を宣言する。

## Scope 外

- OpenAI Responses scheme の新設（`tickets/llm-scheme-openai-responses.md`）
- Codex OAuth 認証アダプタの実装（`tickets/llm-auth-codex-oauth.md`）
- OpenAI 互換ルーター各社の動作確認
- プロバイダ選択 UI（将来の native GUI / TUI 拡張）
