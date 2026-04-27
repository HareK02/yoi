# Review: モデル reasoning/thinking 制御の内部抽象整理

## 前提・要件の確認

- manifest / 上位設定で reasoning を `String` または `i32` として表現できる
  - `ReasoningControl` を `#[serde(untagged)]` enum 化し、`Effort(ReasoningEffort)` と `BudgetTokens(i32)` のいずれかとして deserialize できる
    （`crates/llm-worker/src/llm_client/capability.rs:87-92`）。
  - TOML 経由でも文字列・整数を受けられることを `crates/manifest/src/config.rs:709-731` のテスト
    `from_toml_accepts_worker_reasoning_string_or_integer` が直接確認している。
- `WorkerManifestConfig` / `WorkerManifest` が reasoning を保持し、cascade merge 後に `pod::apply_worker_manifest()` から `RequestConfig::reasoning` へ渡される
  - `WorkerManifestConfig` に `reasoning: Option<ReasoningControl>` を追加（`crates/manifest/src/config.rs:65-69`）。
  - `WorkerManifestConfig::merge` が `upper.reasoning.or(self.reasoning)` で他フィールドと同じ「上位優先」ポリシーで合成
    （`crates/manifest/src/config.rs:228-230`）。テスト `merge_worker_reasoning_upper_wins`（`563-587`）で確認済み。
  - `TryFrom<PodManifestConfig> for PodManifest` が `cfg.worker.reasoning` を `WorkerManifest::reasoning` に転送
    （`crates/manifest/src/config.rs:341-343`）。
  - `WorkerManifest` 自体にも `reasoning` フィールドが追加されている（`crates/manifest/src/lib.rs:103-105`）。
  - `apply_worker_manifest()` が `config.reasoning = wm.reasoning.clone();` で `RequestConfig` に反映
    （`crates/pod/src/pod.rs:1401`）。
- `ReasoningControl` が enum 化され、effort と budget の同時指定が型上できない
  - 同じ enum の 2 variant（`Effort(_)` と `BudgetTokens(_)`）として表現されており、両方を同時に保持する手段は存在しない。
- `ReasoningEffort` が `minimal` / `low` / `medium` / `high` / `xhigh` と未知文字列の素通しを扱える
  - `ReasoningEffort` に 5 つの既知 variant + `Other(String)` を定義
    （`crates/llm-worker/src/llm_client/capability.rs:94-102`）。
  - `From<String>` / 手書き `Deserialize` で未知ラベルが `Other(...)` に落ちる（`117-145`）。
    テスト `reasoning_control_deserializes_effort_labels` が `xhigh` 既知化と `provider-native` 素通しを確認。
- budget token 指定が signed integer として扱われ、Gemini の `-1` のような値を表現できる
  - `ReasoningControl::BudgetTokens(i32)` で signed 化済み。
  - Anthropic 側 wire 型 `AnthropicThinking::Enabled.budget_tokens` も `u32` → `i32` に変更
    （`crates/llm-worker/src/llm_client/scheme/anthropic/request.rs:44`）。
  - Gemini scheme は `as i32` キャストを廃して直接 `i32` を渡す（`crates/llm-worker/src/llm_client/scheme/gemini/request.rs:208-211`）。
  - `parse_reasoning_budget` テストで `-1` がそのまま伝搬することを確認（`crates/manifest/src/lib.rs:380-388`）。
- OpenAI Chat / OpenAI Responses / Anthropic / Gemini の既存 reasoning 投影が enum 型に追従している
  - OpenAI Chat: `Effort(_)` のみ `reasoning_effort` に投影、`BudgetTokens(_)` は黙殺
    （`crates/llm-worker/src/llm_client/scheme/openai_chat/request.rs:154-160`）。
  - OpenAI Responses: 同様に `Effort(_)` のみ `reasoning.effort` へ。`BudgetTokens(_)` のときは
    `effort=None` の `ReasoningConfig` を作りかけて末尾の `.filter(|r| r.effort.is_some())` で除外
    （`crates/llm-worker/src/llm_client/scheme/openai_responses/request.rs:167-179`）。
  - Anthropic: `BudgetTokens(_)` のみ `thinking.budget_tokens` に投影、`Effort(_)` は黙殺
    （`crates/llm-worker/src/llm_client/scheme/anthropic/request.rs:166-178`）。
  - Gemini: `BudgetTokens(_)` のみ `thinking_budget` に投影、`Effort(_)` は黙殺
    （`crates/llm-worker/src/llm_client/scheme/gemini/request.rs:200-211`）。
  - 各 scheme に variant 不一致時の正の・負の両ケースを示すユニットテストが追加されている
    （`thinking_budget_projected_when_supported` / `effort_reasoning_not_projected_to_*` / `reasoning_effort_projected_when_supported` /
    `budget_reasoning_not_projected_to_openai_chat`）。
- 既存の reasoning 無指定時は wire request にパラメータを出さない
  - `request.config.reasoning.as_ref()` 起点なので `None` のときは scheme 投影に入らず、`Option<…>` フィールドは
    すべて `#[serde(skip_serializing_if = "Option::is_none")]` 系で省略される。
    `reasoning_omitted_when_unsupported` テスト（OpenAI Responses）でも追従している。

## アーキテクチャ・スコープ

- 共通型は `llm-worker/src/llm_client/capability.rs` に閉じ、manifest 側は `pub use llm_worker::...::{ReasoningControl, ReasoningEffort}` で
  再エクスポートするだけ（`crates/manifest/src/model.rs:19`）。`ModelCapability` の従来パターンに沿っており、
  `llm-worker は低レベル基盤に留める` 方針と整合する。
- `cargo add` 系の依存追加は発生していない（`serde` 既存のものに `Deserializer`/`Serializer` を追加 import するのみ）。
- 検証は capability の `Effort` / `BudgetTokens` / `Both` 区分どまりで、ラベル文字列や budget 数値そのものは
  provider に委ねている（チケットの「最小限の検証」要件と一致）。
- `WorkerManifestConfig::merge` の挙動が他フィールドと一貫した `upper.or(self)`。新たな policy 分岐を持ち込まず、
  cascade の規則を守っている。
- 範囲外項目（UI プリセット、provider 推奨値テーブル、未実装 scheme への展開、reasoning 出力 block の保存ポリシー）
  はいずれも触れられていない。

## 指摘事項

### Non-blocking / Follow-up

- OpenAI Responses scheme の reasoning 投影
  （`crates/llm-worker/src/llm_client/scheme/openai_responses/request.rs:167-179`）は
  `.map(|effort| ReasoningConfig { effort: match …, … }).filter(|r| r.effort.is_some())`
  という二段構えで「`BudgetTokens` が来たらいったん `effort=None` で組み立ててから捨てる」形になっている。
  動作上は正しく無指定時と同じ wire になるが、`and_then` で `Effort` だけを通す形に揃えると
  Anthropic / Gemini / OpenAI Chat の他 scheme と読み口が一致して読みやすい。
- `ReasoningEffort::Other(String)` を保持するため `ReasoningEffort` 自体は `Copy` を外している。
  これは正しいトレードオフだが、`ReasoningControl` も `Clone` のみとなり、`apply_worker_manifest` で
  `wm.reasoning.clone()` が必要になっている。意図通りなのでそのままでよい（記録のみ）。

### Nits

- `crates/manifest/src/config.rs` 内の `ResolveError::RelativePath`、`flush_pending` 周りの呼び出しなど、
  rustfmt 依存の整形差分が大量に紛れ込んでいる。レビュー対象としては無害だが、commit を分けると
  reasoning 周りの diff が読みやすくなる。今回限りの整形なら現状で問題なし。

## 判断

Approve — 完了条件はすべて満たされており、enum 化により effort と budget の同時指定が型上排除されている。
manifest の `[worker] reasoning =` から `RequestConfig::reasoning`、各 scheme の wire 形式までの経路が
テストで貫通検証されている。OpenAI Responses の二段 filter は読みやすさのみの非ブロッキング指摘。
