# Prune の savings 推定を正確にする

## 背景

現在の PruneHook は `savings_for_drop_impl(context, &snapshot, first..last)` で
候補範囲を「丸ごと drop した場合」の savings を計算し、`min_savings` と比較している。

しかし prune が実際に行うのは ToolResult の content を省略するだけで、
item 自体（summary、メタデータ）は残る。そのため `savings_for_drop` は
実際の節約量を過大評価しており、本来 prune 不要な場面でも発動しうる。

savings の推定は prune 側の責務であり、token_counter の汎用 API に
prune 固有の挙動を押し込むべきではない。

## 方針

PruneHook が「content 部分だけの savings」を計算する。

### 計算方法

候補の各 ToolResult について:
- content ありの item のトークン推定
- content を None にした場合（summary のみ）のトークン推定
- 差分が prune による実際の savings

バイト数の差分を measurement 由来の rate で換算するか、
`tokens_at` を使った前後比較にするかは実装時判断。

### 影響範囲

- `crates/pod/src/prune_hook.rs`: savings 計算ロジックの置き換え
- `crates/pod/src/token_counter.rs`: 必要に応じて content-level の推定ヘルパーを追加

## 依存

- [prune-projection.md](prune-projection.md) — prune が射影ベースになった後の方が設計しやすい

## ブロックする後続

- なし（チューニングの精度改善）
