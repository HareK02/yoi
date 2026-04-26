# メモリ機構: ファイル形式 + Linter 土台

## 背景

`docs/plan/memory.md` で決めたメモリ機構の永続化レイヤの土台。`memory/*` と `knowledge/*` の record を保存・編集する際の静的スキーマと、汎用 CRUD への post-write Hook として挟む Linter を成立させる。Phase 1/2、検索ツール、常駐注入、GC はすべてこの層に乗る。

Workflow（`docs/plan/workflow.md`）も同じ frontmatter / Linter 経路で扱うため、`memory/workflow/<slug>.md` の frontmatter 検証と書き込み制限も本チケットに含める。実行経路（`/<slug>` dispatch）は別。

## 要件

### ディレクトリと record 種別

- `memory/summary.md` — Always-on サマリ（1 ファイル固定）
- `memory/decisions/<slug>.md` — Decisions
- `memory/requests/<slug>.md` — Requests
- `memory/workflow/<slug>.md` — Workflow（frontmatter 検証のみ対象）
- `memory/_staging/<id>.json` — Phase 1 中間（本チケットはパス予約と Linter 対象外化のみ）
- `knowledge/<slug>.md` — Knowledge（`memory/` の兄弟）

slug は kebab-case（小文字英数とハイフン）。ファイル名がそのまま識別子で、frontmatter に `id` / `name` は持たない。

### frontmatter スキーマ

種別ごとの必須フィールドは `docs/plan/memory.md` §ファイル形式 / §書き込み経路と Linter の表に従う。具体的な必須項目：

- 共通: `created_at`, `updated_at`
- Decisions: `sources`, `status: open | resolved | replaced`、置き換え時 `replaced_by: <slug>`
- Requests: `sources`
- Knowledge: `kind`, `description`, `model_invokation`, `user_invocable`, `last_sources`
- Summary: `updated_at`（optional: `last_rewritten_from_range`）
- Workflow: `description`, `auto_invoke`, `user_invocable`, `requires`

### Linter ルール

静的 error（post-write Hook が turn を戻し、sub-Worker に自己修正させる。N 回失敗で abort）:

- frontmatter 必須 field 欠落・型違反
- `memory/workflow/` への書き込み禁止（sub-Worker のみ。人間編集は対象外）
- 同 slug での新規作成禁止（既存があれば update に切り替えるサイン）
- `#<slug>` / `replaced_by: <slug>` / `requires: [..]` が実在 record を指す
- Decisions `status` の enum 違反
- `model_invokation: true` な Knowledge の description 1024 chars 上限
- 種別ごとの char 硬上限（具体値は設定ファイルで tune）

膨張抑制 Warn（error ではなく改善ヒント）:

- 低重要度 × char の天秤
- `sources` 配列長の累積
- 類似 slug 乱立

### 適用経路

- sub-Worker の汎用 CRUD（read/write/edit）への post-write Hook として挟む
- 人間編集（エディタ / git commit）に対しても同一ルールで検証できる CLI または pre-commit hook 経路を用意（詳細は実装で判断、結果として同じルールが一箇所で定義されていること）

## 範囲外

- 検索ツール、常駐注入、Phase 1/2、GC の実装
- 意味破壊（rewrite で主張が落ちる等）の検出 — 監査 LLM 層は将来検討
- staging JSON の schema — Phase 1 チケット
- Workflow の `/<slug>` 実行経路

## 完了条件

- 上記パスに手で record を置いて Linter を走らせると、スキーマ違反 / 参照切れ / 同 slug 競合が error として返る
- sub-Worker が CRUD で書き込んだ際、違反時は turn が戻り自己修正が走る
- Warn は error を止めず、出力されることが確認できる
- `memory/workflow/` への sub-Worker からの書き込みは error で止まり、人間編集は通る
- 既存ビルド・テストを壊さない

## 参照

- `docs/plan/memory.md` §ファイル形式 / §書き込み経路と Linter / §Knowledge の採択基準
- `docs/plan/workflow.md` §格納先とファイル形式 / §生成・更新ポリシー
