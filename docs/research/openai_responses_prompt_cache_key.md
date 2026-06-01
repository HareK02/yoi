# OpenAI Responses API — `prompt_cache_key` Parameter

- **Source**: https://platform.openai.com/docs/api-reference/responses/create
- **Retrieved**: 2026-05-02

---

## 1. パラメータ名

`prompt_cache_key` — Responses API のリクエスト body に載せる任意の文字列キー。
プロンプトキャッシュのスコープを明示する。

## 2. 必須 / 任意

**任意 (optional)**。

公式 OpenAI Responses API では省略しても automatic prefix matching が
走るためキャッシュは効く。一方 ChatGPT backend (codex-oauth /
`https://chatgpt.com/backend-api/codex/responses`) では **明示キーが
無いと事実上ヒットしない**（後述）。

## 3. ChatGPT backend (`codex-oauth`) でのキャッシュ挙動

実観測（`019de419-...` セッション、171 turn / 累計入力 22.2M token）で
`cache_read_tokens` が全 turn 0 だった。最新セッション (`019de48f-...`,
5 turn) でも 0。

原因:

1. ChatGPT backend のプロンプトキャッシュは org / project 単位で
   ハッシュ衝突する設計
2. `prompt_cache_key` を送らないと、複数 conversation のリクエストが
   同じハッシュ空間に積み上がり、prefix が他 conversation で
   上書きされてヒット率が落ちる
3. Codex CLI の実装はこれを認識しており、conversation_id を毎リクエスト
   送って自分専用の名前空間にキャッシュさせている:

```rust
// codex-rs/core/src/client.rs:853
let prompt_cache_key = Some(self.client.state.conversation_id.to_string());
```

## 4. 公式 OpenAI API での挙動

公式エンドポイント (`https://api.openai.com/v1/responses`) では、
明示キーが無くても automatic prefix matching が走る。明示キーを
送ることで、複数 client / 複数 organization が同じ prefix を共有する
シナリオ（マルチテナント等）で意図しないヒット混線を避ける用途
で使う。少なくとも害は無いので両 backend で同じ値を送って良い。

## 5. yoi での運用

- `Request::cache_key: Option<String>` を provider-agnostic な
  キャッシュヒントとして持つ。`cache_anchor` (Anthropic 用 prefix
  index) と並立する別概念。
- `OpenAIResponsesScheme::build_request` で
  `request.cache_key.clone()` を `prompt_cache_key` に投影。
  `None` のときは body にキー自体が載らない
  (`#[serde(skip_serializing_if = "Option::is_none")]`)。
- pod 側は LLM 呼び出し時に `SessionId.to_string()` を渡す。
  主 Run / compactor / extract / consolidate worker のすべてが
  同じ `session_id` を使うので、pod 内の派生 worker が prefix を
  共有しているところでヒットが期待できる。
- 他 scheme (`anthropic`, `gemini`, `openai_chat`) は
  `Request::cache_key` を未参照のまま無視する。

## 6. Fork との関係

`session-store::fork` / `fork_at` はいずれも新 `SessionId` を発行する。
新 fork = 新 cache_key とする（素直に `SessionId.to_string()` を渡す）。

fork 直後の cache 明示ヒットは失われるが、OpenAI Responses は
automatic prefix matching も走るため完全に冷えるわけではない。
fork 越しに親の cache_key を継承して明示ヒットも残す最適化は
別チケット扱い。

## 7. Compaction との関係

compaction は session_id を入れ替える (`create_compacted_session`)。
compact 直後に worker の `cache_key` も新 session_id で更新するため、
post-compact turn は extract / consolidate worker と同じ namespace で
動く。compact 自体は prefix を大幅に書き換えるので、明示キー継続の
有無に関わらずヒット率は元から低い。

## 8. ドキュメント URL

- 公式 API リファレンス: https://platform.openai.com/docs/api-reference/responses/create
- Codex CLI 実装 (conversation_id を prompt_cache_key に渡す):
  https://github.com/openai/codex/blob/main/codex-rs/core/src/client.rs
- ChatGPT backend のサポートパラメータ (LiteLLM issue):
  https://github.com/BerriAI/litellm/issues/21193
