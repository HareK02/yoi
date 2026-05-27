# OpenAI Responses API — `max_output_tokens` Parameter

- **Source**: https://platform.openai.com/docs/api-reference/responses/create
- **Retrieved**: 2026-04-28

---

## 1. パラメータ名

`max_output_tokens` — 正しい。Chat Completions API の `max_tokens` / `max_completion_tokens` とは別物。

## 2. 必須 / 任意

**任意 (optional)**。省略時のデフォルトは `inf`（モデルが許容する最大出力トークン数）。

## 3. 型と範囲

| 項目 | 値 |
|---|---|
| 型 | `integer` または文字列 `"inf"` |
| 最小値 | `1` |
| 最大値 | モデルごとの最大出力トークン数（例: gpt-4.1 系は 32,768） |
| デフォルト | `inf` |

上限に達した場合、レスポンスの `status` が `"incomplete"` になり、`incomplete_details.reason` が `"max_output_tokens"` にセットされる。

## 4. Reasoning tokens との関係 / Reasoning モデルとの組合せ制約

`max_output_tokens` は **reasoning tokens を含む** 合計生成トークン数の上限として機能する。
公式ガイド (https://platform.openai.com/docs/guides/reasoning) には以下の記述がある:

> "You can limit the total number of tokens the model generates (including both reasoning and final output tokens) by using the max_output_tokens parameter."

**実用上の注意点:**
- モデルが内部思考に多数の reasoning tokens を消費した後に上限に達すると、visible output が一切返らずに打ち切られる場合がある。
- コスト制御目的には `reasoning.effort` (`"low"` など) の使用が推奨される。`max_output_tokens` はあくまで暴走抑止のガードとして位置づける。
- o シリーズなど reasoning モデルでは `reasoning.max_tokens` (別パラメータ) で reasoning 専用の上限を設定できる場合もある。

## 5. Codex CLI 互換 Responses 経路における取り扱い

この経路は公式 Responses API のパラメータをすべて受け付けるわけではなく、`max_output_tokens` を **サポートしないパラメータとして 400 エラーで拒否する**。

LiteLLM の調査 (https://github.com/BerriAI/litellm/issues/21193) によれば、この経路が受け付けるパラメータは以下に限られる:

```
model, input, instructions, stream, store, include,
tools, tool_choice, reasoning, previous_response_id, truncation
```

`max_output_tokens`, `max_tokens`, `max_completion_tokens`, `temperature`, `top_p`, `user`, `metadata`, `context_management` はすべて拒否される（実観測でも `temperature` 同梱リクエストは `{"detail":"Unsupported parameter: temperature"}` を返す）。

Codex CLI 自身も `config.toml` の `model_max_output_tokens` を API リクエストに載せない実装になっており (https://github.com/openai/codex/issues/4138)、これはバグではなく ChatGPT backend の制約に対する回避策と解釈できる。同 CLI は `temperature` / `top_p` も送出しない。

本リポジトリでは `OpenAIResponsesScheme` の `send_max_output_tokens` / `send_sampling_params` フラグでこれらの送出を一括制御し、`provider/src/lib.rs` 内で `AuthRef::CodexOAuth` 指定時に両方 `false` にする。

## 6. ドキュメント URL

- 公式 API リファレンス: https://platform.openai.com/docs/api-reference/responses/create
- Reasoning ガイド: https://platform.openai.com/docs/guides/reasoning
- Codex CLI issue (max_output_tokens 未送信): https://github.com/openai/codex/issues/4138
- LiteLLM issue (ChatGPT backend 拒否パラメータ一覧): https://github.com/BerriAI/litellm/issues/21193
- OpenAI Community (reasoning tokens 上限): https://community.openai.com/t/limiting-maximum-number-of-reasoning-tokens/1285430
