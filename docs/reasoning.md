# Reasoning / Thinking 制御

manifest の `[worker]` セクションで `reasoning` を指定すると、scheme が provider 各社の wire 形式に投影する。文字列なら **effort label**、数値なら **thinking budget tokens** として扱う。insomnia 側は値の妥当性を検証せず、未知ラベルや provider が拒む値は API 応答で初めて検出される。

## 書き方

```toml
[worker]
reasoning = "medium"   # effort label
```

```toml
[worker]
reasoning = 4096       # thinking budget tokens (i32)
```

未指定なら wire request に reasoning / thinking 関連フィールドは出さない。

## Provider ごとの受け入れ形式

| Provider / scheme | 受け入れる形式 | 投影先 |
|---|---|---|
| OpenAI Chat Completions (`openai_chat`) | effort label のみ | `reasoning_effort` |
| OpenAI Responses (`openai_responses`) | effort label のみ | `reasoning: { effort, summary: "auto" }` |
| Anthropic (`anthropic`) | budget tokens のみ | `thinking: { type: "enabled", budget_tokens }` |
| Gemini (`gemini`) | budget tokens のみ | `generation_config.thinking_config.thinking_budget` |

`ModelCapability::reasoning` (`ReasoningSupport::{Effort, BudgetTokens, Both}`) と request 側の variant が一致しないときは、その scheme は wire に何も載せない（capability gating）。例: Anthropic に `reasoning = "medium"` を渡しても黙って drop される。

## Effort label

`ReasoningEffort` の既知 variant は `minimal` / `low` / `medium` / `high` / `xhigh`。これら以外の文字列は `Other(String)` として provider にそのまま渡る（OpenAI 側の独自ラベルや将来追加に対応）。

## Budget tokens

signed integer (`i32`) として扱う。Gemini の `-1`（dynamic budget）のような特殊値も型変換なしで通る。範囲チェックは provider に任せる。

## 設定例

OpenAI o-series:

```toml
[model]
ref = "openai/gpt-5"

[worker]
reasoning = "high"
```

Anthropic extended thinking:

```toml
[model]
ref = "anthropic/claude-sonnet-4-6"

[worker]
reasoning = 8192
```

Gemini dynamic thinking:

```toml
[model]
ref = "gemini/gemini-2.5-pro"

[worker]
reasoning = -1
```

## 範囲外

- UI プリセット（Low / Medium / High → 各 provider 値）の変換テーブル
- provider ごとの推奨 budget レンジ
- reasoning / thinking 出力 block のログ・再送・表示ポリシー
