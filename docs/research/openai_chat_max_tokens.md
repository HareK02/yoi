# OpenAI Chat Completions API — 出力トークン数制御パラメータ仕様

- **Source**: https://platform.openai.com/docs/api-reference/chat/create
- **Supplementary**: https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/reasoning
- **Retrieved**: 2026-04-28

---

## 1. `max_tokens` と `max_completion_tokens` の関係

| パラメータ | 状態 | 対応モデル |
|---|---|---|
| `max_tokens` | **Deprecated** | GPT-3.5, GPT-4 系など旧モデルでは動作する |
| `max_completion_tokens` | 現行・推奨 | 全モデル（旧モデルにも後方互換あり） |

- `max_tokens` は o1 系以降では **受け付けられない**（エラーまたは無視）。
- 変更の背景: 旧来の `max_tokens` は「返却トークン = 生成トークン = 課金トークン」を前提にしていた。o1 系で推論トークン（reasoning tokens）が導入されたことでこの前提が崩れ、新パラメータが設計された。
- `max_completion_tokens` は旧モデルでも機能するため、**新規実装では `max_completion_tokens` を使うべき**。

## 2. 必須か任意か

- **任意（optional）**。
- 指定しない場合はモデルのコンテキスト上限まで生成する（デフォルト: `null`）。

## 3. 型と範囲

- **型**: `integer | null`
- **範囲**: `1` 以上、モデルのコンテキストウィンドウの残りトークン数以下。上限値はモデルごとに異なり、ドキュメント上に固定の最大値は明示されていない。
- `null` を渡すと制限なし（モデル上限に従う）。

## 4. Reasoning モデルでの reasoning tokens のカウント

- `max_completion_tokens` の上限には **reasoning tokens（推論トークン）を含む**。
  - reasoning tokens: モデルが内部で生成するが API レスポンスには含まれない隠しトークン。
  - 課金対象は reasoning tokens + visible output tokens の合計。
  - レスポンスの `usage.completion_tokens_details.reasoning_tokens` で内訳を確認できる。
- したがって、`max_completion_tokens = 5000` と設定しても、推論に多くのトークンを使った場合、目に見える出力は 5000 より少なくなる。

## 5. Ollama の OpenAI compat API での扱い（補助情報）

- Ollama の `/v1/chat/completions` は現時点で **`max_tokens` のみを公式サポート**している（内部的に `num_predict` にマッピング）。
- `max_completion_tokens` サポートは Issue #7125 / PR #14464 で議論中だが、2026-04-28 時点では公式ドキュメント上に記載なし。
- **Ollama に対しては `max_tokens` を使う**のが安全な選択。ただし将来的に `max_completion_tokens` に移行される見込み。

## 6. ドキュメント URL

- [OpenAI Chat Completions API Reference](https://platform.openai.com/docs/api-reference/chat/create)
- [Azure OpenAI Reasoning Models (GPT-5, o3, o1)](https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/reasoning)
- [Ollama OpenAI Compatibility](https://docs.ollama.com/api/openai-compatibility)
- [Ollama Issue #7125 — max_completion_tokens support](https://github.com/ollama/ollama/issues/7125)
- [OpenAI Community — Why max_tokens changed to max_completion_tokens](https://community.openai.com/t/why-was-max-tokens-changed-to-max-completion-tokens/938077)
