# メモリ機構: GC（定期再評価）

## 背景

`docs/plan/memory.md` §GC の実装。Phase 2 は情報統合寄りだが、それでも残る重要度低下・類似 slug 乱立・`replaced` 滞留・sources 累積・現状不整合を整理する定期再評価経路。人間 offer はかけず、結果は git diff で検証する建て付け。

保護閾値は使用頻度メトリクスの明示 invoke frequency に依存する。

## 要件

### Trigger

- 定期実行（累積 input token ベース推奨、具体値は設定で tune、実装判断）

### 実行主体と渡すツール

- GC Agent が spawn
- Phase 2 と同じ汎用 CRUD + 検索ツール + post-write Linter Hook
- 入力: GC 対象 record 群 + Linter Warn + 使用頻度メトリクス + `replaced` chain + sources 過多情報

### 操作粒度

- ファイル単位: 丸ごと drop / 複数ファイル merge / 1 ファイルの split
- ファイル内部分: 節・箇条の削除 or 圧縮、`sources` 古いエントリの trim

### 評価カテゴリ

`outdated`, `superseded`, `unused`, `noisy` を GC 理由の分類として使う。record に一律の「stale」フラグは付けない。drop / merge / split / trim / rewrite のどれを選ぶかをこのカテゴリで説明可能にする。

### 判断ルール

- 保護閾値: **明示 invoke** の `frequency >= 1.0 invokes/Mtoken` の record は drop / 大幅圧縮の対象外
  - 初期値 1.0、workspace 設定でカスタマイズ可
  - `model_invokation` 注入による常駐は計数対象外（別指標として参照のみ）
- 単一 record が複数カテゴリに該当してもよい
- 直接削除してよいが、誤判定しやすいものは merge / trim を優先（prompt 側で誘導）

### prompt

- `docs/plan/memory-prompts.md` §GC prompt に従う

## 範囲外

- 監査 LLM 層（将来検討）
- Vector index / FTS5 導入（将来検討、GC 判断には影響しない）
- Workflow の GC 対象化（初期は触らない）

## 完了条件

- Trigger で GC Agent が走り、不要 record が整理される
- Linter Warn で検出した類似 slug 乱立などが GC でまとめて収斂する
- 保護閾値超過 record が drop / 大幅圧縮から外れる
- 置き換えは `status: replaced` + `replaced_by` で残り、直接削除と区別可能
- git diff で drop / merge / split / trim / rewrite の理由が読める

## 参照

- `docs/plan/memory.md` §GC / §使用頻度メトリクス / §判断ルール
- `docs/plan/memory-prompts.md` §GC prompt
- `tickets/memory-phase2-consolidation.md`（ツール構成の共通化）
- `tickets/memory-usage-metrics.md`（保護閾値の依存）
