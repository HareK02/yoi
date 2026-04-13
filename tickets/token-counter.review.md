# token-counter レビュー

## 要件の充足

チケットが定義した 3 API・型・アルゴリズムは全て実装されている:

- `Pod::total_tokens()` → `TokenEstimate`
- `Pod::split_for_retained(retained)` → `SplitPoint`
- `Pod::savings_for_drop(range)` → `TokenEstimate`
- `EstimateSource`: `Measured / Interpolated / Extrapolated / NoData`

設計方針（状態を持たない pure 関数、provider 非依存、ローカルトークナイザ不要）も
満たされている。`_impl` 関数群は `(&[Item], &[UsageRecord])` だけを受け取り、
Pod メソッドは history と usage_history を渡すだけの薄いラッパー。

## アーキテクチャ

| レイヤー | 変更内容 |
|---------|---------|
| pod::token_counter | pure な計算関数 + Pod のメソッドとして公開 |
| pod::Pod | `usage_history: Arc<Mutex<Vec<UsageRecord>>>` を追加。restore で復元、persist_turn で追記、compact で clear |
| pod::PruneHook | min_savings 判定を `savings_for_drop_impl` に委譲。usage_history の shared handle を保持 |
| llm-worker::prune | `prune()` → `prunable_indices()` + `apply_prune()` に分解。min_savings 判定とトークン会計への依存を除去 |

prune の責務分離が適切。llm-worker 側は pure な候補抽出と適用のみ、
トークン会計への依存は pod 層に閉じている。

## 指摘と対処

### 1. split_for_retained_impl の O(n²) シリアライズ（非ブロッカー、未対処）

`tokens_at` を `1..=history.len()` で毎回呼び、内部で `prefix_bytes`（history 全体の
JSON シリアライズ）を都度計算。長大セッションでは item 数に対して二乗になる。
`prefix_bytes` をループ外で 1 回だけ計算して渡す形にリファクタリングすべきだが、
現時点の history サイズでは実害なし。パフォーマンスが問題になった段階で対処。

### 2. PruneHook の savings 過大評価（認識済み、未対処）

prune は content を None にするだけで item を消さないため、`savings_for_drop`
（範囲全体の drop を仮定）は実際の節約量より大きい値を返す。閾値判定としては
prune を発動しやすい方向＝安全側。ログの `estimated_savings_tokens` が過大になる
点はチューニング時に注意。

### 3. compact 後の usage_history.clear()（後続チケットで対処）

compact 直後は measurement が空になり `total_tokens()` が `NoData` を返す。
compact-improvements で `last_input_tokens` を撤去して閾値判定を usage 経由に
一本化する際、この NoData 期間の扱いを設計する必要がある。

## テスト

token_counter: 13 件（NoData / Measured / Extrapolated / Interpolated 各ケース、
split の境界、savings の measurement 差分、空 range、out-of-range）。

prune (llm-worker): `prunable_indices` + `apply_prune` に分解後のテスト 5 件。
候補抽出、適用、冪等性、既 prune 済み除外、境界。

## 判定

承認。
