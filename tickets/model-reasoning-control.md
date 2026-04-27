# モデル reasoning/thinking 制御の内部抽象整理

## 背景

`llm-worker` には `RequestConfig::reasoning` と `ReasoningControl { effort, budget_tokens }` があり、各 scheme も OpenAI 系の `reasoning_effort` / `reasoning.effort`、Anthropic の `thinking.budget_tokens`、Gemini の `thinking_config.thinking_budget` へ投影する実装を持っている。

一方で、上位層から通常のリクエストへ `reasoning` を設定する経路はまだ整っておらず、内部型も `effort: Option<_>` と `budget_tokens: Option<_>` を同時に持てるため、「今回の指定がラベルなのか数値 budget なのか」が型で一意に表現されていない。

Provider ごとの正当値は変化しやすく、OpenAI 系は `low` / `medium` / `high` などの文字列 effort、Anthropic / Gemini 系は token budget 数値、Gemini には `-1` dynamic budget のような値がある。insomnia はこれらを過度に正規化せず、上位層・manifest では `String` または `i32` として受け、内部では「文字列 effort か数値 budget か」を enum で保持し、scheme が自身の wire 形式へ投影する。

## 要件

### manifest / 上位設定での表現

reasoning/thinking 制御は、上位層および manifest で `String` または `i32` として指定できるようにする。

例:

```toml
reasoning = "medium"
```

```toml
reasoning = 4096
```

文字列は provider-native な effort label として扱い、数値は provider-native な thinking budget token 数として扱う。間違った値や provider が受け付けない組み合わせは insomnia 側で過度に検証せず、原則として provider API のエラーに任せる。

現状は `WorkerManifestConfig` / `WorkerManifest` に `reasoning` フィールドが無く、`pod::apply_worker_manifest()` も `RequestConfig::new()` に `max_tokens` と `temperature` だけを移しているため、通常の Pod/manifest 起動経路では `RequestConfig::reasoning` が常に `None` のまま scheme へ届く。このチケットでは `[worker]` 由来の reasoning 指定を `RequestConfig` へ渡す経路まで含める。

### 内部型の enum 化

`ReasoningControl` は `effort` と `budget_tokens` を同時に持つ struct ではなく、指定種別が一意な enum に整理する。

想定形:

```rust
pub enum ReasoningControl {
    Effort(ReasoningEffort),
    BudgetTokens(i32),
}
```

`ReasoningEffort` は既知値を variant として持ちつつ、未知の provider-native label を素通しできるようにする。

想定形:

```rust
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Other(String),
}
```

数値 budget は Gemini の `-1` dynamic budget 等を表現できるよう、`u32` ではなく signed integer とする。

### scheme 投影

各 scheme は `ReasoningControl` の variant と `ModelCapability::reasoning` を見て、自身が wire に載せられる形式のみ投影する。

- OpenAI Chat Completions: `Effort(_)` を `reasoning_effort` に投影する
- OpenAI Responses: `Effort(_)` を `reasoning: { effort, summary: "auto" }` に投影する
- Anthropic: `BudgetTokens(_)` を `thinking: { type: "enabled", budget_tokens }` に投影する
- Gemini: `BudgetTokens(_)` を `generation_config.thinking_config.thinking_budget` に投影する

provider-native な値そのものの妥当性検証は最小限に留める。例えば OpenAI に未知 effort label を送る、Anthropic に provider が許容しない budget を送る、といったケースは provider API の応答で検出されればよい。

### Capability との関係

`ModelCapability::reasoning` は、その model/provider が受けられる reasoning 指定の大まかな形式を表す既存の `ReasoningSupport::{Effort, BudgetTokens, Both}` を維持してよい。

ただし capability は正当値リストではなく、scheme 投影の可否判定に留める。モデル別の厳密な effort label や budget range を insomnia 側で網羅しない。

## 範囲外

- UI 上のプリセット（Low / Medium / High 等）をどの値へ変換するかの設計
- provider ごとの budget 推奨値テーブル
- OpenRouter / Groq / DeepSeek 等、現行 scheme で未実装の reasoning 統一 API への追加対応
- reasoning / thinking 出力 block のログ保存・再送・表示ポリシーの変更

## 完了条件

- manifest / 上位設定で reasoning を `String` または `i32` として表現できる
- `WorkerManifestConfig` / `WorkerManifest` が reasoning 指定を保持し、cascade merge 後に `pod::apply_worker_manifest()` から `RequestConfig::reasoning` へ渡される
- `ReasoningControl` が enum 化され、effort と budget の同時指定が型上できない
- `ReasoningEffort` が `minimal` / `low` / `medium` / `high` / `xhigh` と未知文字列の素通しを扱える
- budget token 指定が signed integer として扱われ、Gemini の `-1` のような値を表現できる
- OpenAI Chat / OpenAI Responses / Anthropic / Gemini の既存 reasoning 投影が enum 型に追従している
- 既存の reasoning 無指定時は、従来通り wire request に reasoning/thinking パラメータを出さない
