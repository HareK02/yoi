# メモリ機構: 使用頻度メトリクス + Knowledge 化候補レポート

## 背景

`docs/plan/memory.md` §使用頻度メトリクス の実装。memory 検索ツール / Knowledge 検索ツール内に invoke 計測フックを入れ、時間単位ではなく累積 input token で正規化した頻度を算出する。consolidation 統合 step の Knowledge 新規作成 gate と consolidation 整理 step の保護閾値の両方で使われる。

## 要件

### 観測経路

- memory 検索ツール / Knowledge 検索ツール内に hook を挿入
- `#<slug>`（slug 完全一致経路）、`/<slug>`（workflow 側、将来接続）、明示検索呼び出しをすべて同じ経路に集約

### カウント対象

- **明示 invoke**: 検索ツール経由の読み取り / `#<slug>` / `/<slug>` を `n回 / Mtoken` でスコア化
- **`model_invokation` 注入**: 明示 invoke の分子には含めない。**コスト側**（注入した record に対する消費 input tokens）として別途記録。使われ率 ratio や ON/OFF 判断材料として後段で参照
- ファイル token 数

### 記録先

- staging と独立
- workspace 側に記録（session データ喪失で統計が消えない）
- invoke event を UUID + Stats 形式
- 具体 schema / フォーマットは実装で決定

### 集計

- 累積方式: 最大 10 回前の invoke から現在までの時系列窓でフィルタして集計
- **Knowledge 化候補レポート**:
  - 対象は `memory/*` 配下の record（extract 成果物の decisions / requests + 既存 knowledge）
  - 明示 invoke 頻度が閾値超過のものを列挙
  - 同一 session 内の連続参照は 1 count に丸める
  - 複数 session での再参照を要件とする（spike 除外）
  - 閾値は設定ファイルで tune

### 消費者

- consolidation Worker の統合 step 入力として候補レポートを渡す
- consolidation Worker の整理 step で保護閾値判定（明示 invoke frequency >= 1.0 invokes/Mtoken）に使う

## 範囲外

- consolidation 整理 step の実装本体（`memory-consolidation` 側。本チケットは保護閾値判定に必要なメトリクスの提供まで）
- `model_invokation` ON/OFF の自動判定ロジック（将来検討）
- Shallow request の自動除外（将来検討）

## 完了条件

- 検索ツール呼び出しで invoke event が workspace 側に積まれる
- `model_invokation` 注入のコスト側集計が別口で積まれる
- 候補レポート API が consolidation Worker の起動時に呼べる
- 閾値未満の record は候補レポートに載らない
- 同一 session 内連続参照は 1 count に丸まる

## 参照

- `docs/plan/memory.md` §使用頻度メトリクス / §判断ルール / §retrieval 経路
- `tickets/memory-search-tools.md`（hook 挿入点）
- `tickets/memory-consolidation.md`（統合 / 整理 両 step の消費者）
