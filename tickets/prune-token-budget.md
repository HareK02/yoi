# Prune: 保護境界を token budget 化

## 背景

現状の Prune は `prune_protected_turns`（デフォルト 3）で user message 起点の turn 数を数え、直近 N turn 分の `Item::ToolResult.content` を保護する（`crates/llm-worker/src/prune.rs:107-164`、`crates/manifest/src/defaults.rs:13-15`）。

この境界定義は対話頻度に依存して挙動が極端に変わる:

- 短い対話中心のセッション: 4 turn 目以降は意図通り定常的に刈れる。
- 単発の長タスク（agentic loop で 1 user message から数十〜数百 LLM call が走るケース）: history 全体が 1 turn 扱いになり、`turn_starts.len() <= protected_turns` で候補抽出すら行われず、`SkippedNoCandidates` で恒常的に発火しない。コンテキスト窓が一方的に膨れて compaction に押し付ける形になる。

そもそも prune は「古い tool_result の content を切り詰めて token を回収する」機構であり、保護量も token で測るのが意味論的に一貫している。compaction 側は既に `compact_retained_tokens: 8000` という末尾 token budget で保護しており、二つの機構の保護軸を揃えると設定の理解が単純になる。

LLM call 境界（assistant 出力の単位）を history から後追い検出する必要は無い。Prune が走るのは LLM call 直前のみで、その時点で Worker は usage 履歴を持っており、末尾からの累計トークンで境界を引ける。

## 方針

`prune_protected_turns` を撤廃し、`prune_protected_tokens: u64` に置き換える。

- 候補抽出: history 末尾から item ごとの推定トークンを累計し、累計が `protected_tokens` を超える位置までを保護領域とする。それより前の `Item::ToolResult { content: Some(_), .. }` が prune 候補。
- usage 測定が無い時点（最初の LLM call 前 / compact 直後）は推定が `NoData` を返すため、保護領域の決定もできない。この場合は候補抽出を諦めて `SkippedNoCandidates` 相当で抜ける（既存 NoData ハンドリングの踏襲）。
- `prune_min_savings` 判定はそのまま残す。二段の判定（候補があるか / savings が閾値を超えるか）は維持する。
- tool_call と tool_result のペアは `call_id` で対応が取られているため、保護境界が途中を切っても projection は content を `None` にするだけで summary 構造は壊れない。境界の精度よりも token 量の正確さを優先してよい。

## 要件

- Manifest: `compaction.prune_protected_turns` を撤廃し `compaction.prune_protected_tokens: u64` を追加する。後方互換 shim は入れない。
- デフォルト: `PRUNE_PROTECTED_TOKENS = 8000`（`COMPACT_RETAINED_TOKENS` と揃える）。
- `PruneConfig` も同様に `protected_turns` → `protected_tokens` に rename。
- 候補抽出ロジック (`prune.rs`) は token 累計ベースに切り替える。usage 推定の取得経路は既存 `SavingsEstimator` と同じ「Worker に callback を install する」パターンで足す。Worker / prune.rs 自体は usage source を知らないままに保つ。
- メトリクス: `prune.fire` / `prune.skip` の既存 dimension のうち `border_turn` は意味を失うので、保護境界を表す新 dimension（保護領域の先頭 item index または保護領域の累計トークン）に差し替える。`candidate_count` と `value=estimated_savings` は維持する。
- 既存テスト (`prune.rs` 末尾のユニットテスト群) は token budget ベースに書き直す。

## 完了条件

- 単発の長タスクで重い ToolResult が積もるシナリオで、4 番目以降の LLM call から `prune.fire` が観測される（重い ToolResult ほど早く刈られる挙動）。
- 短い対話セッションでも、末尾 8000 token に収まらなくなった古い ToolResult の content が従来通り刈られる。
- `prune_protected_turns` を旧フィールド名で書いた manifest は明示的にエラーになる（後方互換無し）。

## 範囲外

- `Item` に `request_seq` 等の LLM call 境界情報を埋め込む案。今回は履歴構造を変更しない。
- compaction 側 (`compact_retained_tokens`) のロジック変更。
- `prune_min_savings` の値・判定ロジックの変更。
- token 推定アルゴリズム自体の改善（`UsageHistory` ベースの既存推定をそのまま使う）。

## 影響範囲

- `crates/manifest/src/defaults.rs`: `PRUNE_PROTECTED_TURNS` 削除、`PRUNE_PROTECTED_TOKENS` 追加。
- `crates/manifest/src/{config,lib}.rs`: `CompactionConfig` の field rename とカスケード解決の差し替え。
- `crates/llm-worker/src/prune.rs`: `PruneConfig`、`prunable_indices` / `evaluate_candidates` の引数とロジック、ユニットテスト。
- `crates/llm-worker/src/worker.rs`: prune 評価呼び出し箇所、必要なら新しい token-estimator callback の install 経路。
- `crates/pod/src/compact/prune.rs`: `PruneConfig` 組み立てと、新しい token 推定 callback の注入。
- `crates/pod/src/compact/token_counter.rs`: 末尾累計トークン算出のヘルパー（`savings_for_prune_impl` の隣に追加 or 共通化）。
- `crates/pod/tests/session_metrics_test.rs`: `prune.fire` / `prune.skip` の dimension 期待値。
- `docs/compaction.md`: 設定セクションの記述を更新。
