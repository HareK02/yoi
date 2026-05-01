# OpenAI Responses: prompt_cache_key 送出によるキャッシュ有効化

## 背景

codex-oauth 経由 (ChatGPT backend) の OpenAI Responses API において、
プロンプトキャッシュが事実上効いていない。実セッションログ
(`019de419-...`, 171 turn / 累計入力 22.2M token) で `cache_read_tokens`
が全 turn 0、最新セッション (`019de48f-...`, 5 turn) でも 0。

直近のパース修正 (`events.rs` の `ResponsesUsage` に
`input_tokens_details.cached_tokens` を追加) で計測経路は復旧したが、
それでも 0 が観測される → **server 側でそもそもキャッシュが効いていない**。

原因は codex-rs の実装で確定:

```rust
// codex-rs/core/src/client.rs:853
let prompt_cache_key = Some(self.client.state.conversation_id.to_string());
```

ChatGPT backend では `prompt_cache_key` をリクエストに含めないと
プロンプトキャッシュが期待通りに動かない (org/project ハッシュが
別 conversation と衝突しやすく、ヒット率が著しく落ちる)。Codex は
conversation 単位の安定キーを毎リクエスト付けて、その名前空間内で
prefix をキャッシュさせている。

insomnia 側の `ResponsesRequest` には `prompt_cache_key` フィールドが
存在せず、`Request` 構造体にも会話/セッション単位の安定キー概念が無い。
このため codex-oauth で長尺の Run を走らせると毎 turn 全 prefix を
従量課金している。

## 方針

`Request` に provider-agnostic な `cache_key: Option<String>` を
足し、`OpenAIResponsesScheme` がそれを `prompt_cache_key` として
送る。pod 側は LLM 呼び出し時に `SessionId` をキーとして渡す。
他 scheme (Anthropic / Gemini / OpenAI Chat / Ollama) はフィールドを
無視する。既存の `cache_anchor` (Anthropic 用 prefix anchor) と
同じ「キャッシュヒントを Request に載せ、効く provider だけ拾う」
規約に揃える。

### Fork との関係

`session-store::fork` / `fork_at` はいずれも新 `SessionId` を発行する。
本チケットでは **新 fork = 新 cache_key** とする (素直に
`SessionId.to_string()` を渡す)。fork 直後の cache 明示ヒットは失われる
が、OpenAI Responses は automatic prefix matching も走るため完全に
冷えるわけではない。fork 越しに親の cache_key を継承して明示ヒットも
残す最適化は別チケットで検討する (本ticketの範囲外)。

## 要件

### llm-worker 側

- `Request` に `cache_key: Option<String>` を追加 (`types.rs:442`
  の `cache_anchor` の隣)。doc コメントで「会話単位の安定キー。
  prompt_cache_key として送られる (OpenAI Responses)。
  prefix anchor を持たない provider は無視」を明記
- ビルダ `Request::cache_key(impl Into<String>)` を追加
- `OpenAIResponsesScheme::build_request` で `request.cache_key.clone()`
  を `ResponsesRequest::prompt_cache_key` にセット
- `ResponsesRequest` に `prompt_cache_key: Option<String>` を追加
  (`#[serde(skip_serializing_if = "Option::is_none")]`)
- 他 scheme (`anthropic`, `gemini`, `openai_chat`) は touch しない
  (Request の新フィールドを未参照のまま残す)

### pod 側

- LLM クライアントに渡す `Request` を組み立てる箇所で
  `cache_key(session_id.to_string())` を入れる。少なくとも以下:
  - 主 Run の LLM 呼び出し (`pod.rs` の Run / Worker 経路)
  - compactor worker
  - memory extract worker
- `SessionId` は `SharedState::session_id` から取得できる
  (`shared_state.rs:21`)
- compactor / extract のように pod の中で派生する worker でも
  同じ `session_id` を使う。これにより pod 内のすべての LLM
  呼び出しが同一 cache_key 名前空間で動き、prefix が共有される
  ところでヒットが期待できる

### docs

- `docs/research/` 配下に `openai_responses_prompt_cache_key.md`
  (仮) を追加し、「ChatGPT backend では prompt_cache_key 必須」
  「codex-rs の挙動」「insomnia での Fork 方針」を残す。
  既存の `openai_responses_max_output_tokens.md` と並びで置く

## 完了条件

- `Request::cache_key("abc")` で組んだリクエストが、
  `OpenAIResponsesScheme::build_request` で
  `prompt_cache_key: "abc"` を含む body を生成する (unit test)
- `cache_key = None` のときは body に `prompt_cache_key` キーが
  載らない (`skip_serializing_if`) (unit test)
- pod の Run で codex-oauth + Responses を使ったとき、2 turn 目
  以降の `cache_read_tokens` が 0 でない (実セッションログで確認)
- `cargo check` / `cargo test` が `llm-worker`, `provider`, `pod`
  で通る

## 範囲外

- Fork 越しのキャッシュ継承 (`forked_from` を辿って root の
  cache_key を継承する最適化)。別チケット
- 公式 OpenAI Responses API (非 ChatGPT backend) での
  `prompt_cache_key` 必要性検証。少なくとも害は無いので両経路で
  同じ値を送って良い
- compaction で prefix が大きく書き換わる経路の cache_key 戦略
  (compaction 後は prefix がほぼ別物なので、ヒット率を最大化する
  なら compaction 直後だけ別 key にする手もあるが、まずは単純に
  session_id 一本で動かす)
- `cache_anchor` (Anthropic 用) と `cache_key` (Responses 用) の
  統合。両者は別概念 (前者は prefix の境界 index、後者は
  名前空間キー) なので並立させる

## Review
- 状態: Approve
- レビュー詳細: [./responses-prompt-cache-key.review.md](./responses-prompt-cache-key.review.md)
- 日付: 2026-05-02
