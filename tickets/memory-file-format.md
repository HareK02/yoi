# メモリ機構: ファイル形式 + Linter 土台

## 背景

`docs/plan/memory.md` で決めたメモリ機構の永続化レイヤの土台。`memory/*` と `knowledge/*` の record を保存・編集する際の静的スキーマと、書き込み時の Linter を成立させる。Phase 1/2、検索ツール、常駐注入、GC はすべてこの層に乗る。

Workflow（`docs/plan/workflow.md`）も同じ frontmatter / Linter 経路で扱うため、`memory/workflow/<slug>.md` の frontmatter 検証と書き込み制限も本チケットに含める。実行経路（`/<slug>` dispatch）は別。

## 設計方針

### memory クレートに集約

memory 関連は新規 `crates/memory/` に全部閉じ込める。`tools` クレートや `pod` 層に memory 由来のコードを漏らさない。

- schema（frontmatter 型）、slug 文法、Linter ルール、Tool 実装、ワークスペース解決を全部 `memory` クレート内に置く
- `tools::write_tool` / `edit_tool` には一切手を入れない
- LLM への違反伝達は memory tool が `ToolError::InvalidArgument` を返すことで自然に成立。Interceptor 拡張・retry message 注入・違反カウンタは持たない
- 「N 回失敗で abort」は worker 層の max iteration に委ね、memory 固有のカウンタは設けない

### memory 専用 Tool（汎用 CRUD ではない）

`memory` クレートが `read_tool` / `write_tool` / `edit_tool` の 3 種を提供する。これらは `<workspace>/memory/`、`<workspace>/knowledge/` 配下のみを対象とし、write/edit は **fs 書き込み前** に Linter を通して違反は `ToolError::InvalidArgument` で返す。

`docs/plan/memory.md` の「同じ汎用 CRUD」記述は本チケットで `memory` 専用 Tool 方式に書き換える（汎用 CRUD は memory ディレクトリには触らせない）。

### Pod 側の責務（最小限）

memory を有効化する Pod は、generic tool に渡す Scope から `memory/`、`knowledge/` を deny に落とす。これにより同じ workspace 内で、generic write/edit は memory 配下を触れず、memory tool だけが触れる構造になる。Pod 側でやることはこの Scope deny と memory tool の登録だけ。

### 「sub-Worker / 人間」の二系統

- **sub-Worker**: tool 層を経由するため Linter を必ず通る
- **人間**: エディタ / git commit は tool 層を経由しないので Linter を通らない。`memory::Linter` を import して走らせる CLI / pre-commit hook を後で用意できる構造にしておく（本チケットでは実装しない）

## 要件

### ディレクトリと record 種別

- `memory/summary.md` — Always-on サマリ（1 ファイル固定）
- `memory/decisions/<slug>.md` — Decisions
- `memory/requests/<slug>.md` — Requests
- `memory/workflow/<slug>.md` — Workflow（frontmatter 検証のみ対象）
- `memory/_staging/<id>.json` — Phase 1 中間（パス予約のみ。Linter 対象外）
- `knowledge/<slug>.md` — Knowledge（`memory/` の兄弟）

slug 文法: `^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$`（agent-skills 準拠、1-64 chars、先頭末尾 `-` 不可、`--` 連続不可）。ファイル名がそのまま識別子で、frontmatter に `id` / `name` は持たない。

### frontmatter スキーマ

- 共通: `created_at`, `updated_at`（RFC3339）
- Decisions: `sources`, `status: open | resolved | replaced`、置き換え時 `replaced_by: <slug>`
- Requests: `sources`
- Knowledge: `kind`, `description`, `model_invokation`, `user_invocable`, `last_sources`
- Summary: `updated_at`（optional: `last_rewritten_from_range`）
- Workflow: `description`, `auto_invoke`, `user_invocable`, `requires`

`sources` / `last_sources` の要素形式は `{ session_id: String, range: [u64, u64] }`。`range` は session-store の entry index ペア。

### Linter ルール

**静的 error**（memory tool が `ToolError::InvalidArgument` で返す。複数違反は集約して 1 回で返す）:

- frontmatter 必須 field 欠落・型違反
- `memory/workflow/` への書き込み禁止（sub-Worker のみ。memory tool が拒否、人間編集は memory tool を通らないので素通り）
- 同 slug での新規作成禁止（既存があれば update を要求）
- `replaced_by: <slug>` / `requires: [..]` が実在 record を指す
- `replaced_by` の循環は error
- Decisions `status` の enum 違反
- slug 文法違反
- `model_invokation: true` な Knowledge の description 1024 chars 上限
- 種別ごとの char 硬上限（初期既定値、設定 key は別 PR）:
  - `summary.md`: 20000 chars
  - decisions / requests / knowledge 本文: 各 8000 chars

**膨張抑制 Warn**（error にせず、warn として返す。memory tool は受け取って summary に追記する程度）:

- 低重要度 × char の天秤（暫定: 1500 chars 超のレコードに対して `sources` 1 件のみなら warn）
- `sources` 配列長の累積（暫定: 10 件超で warn）
- 類似 slug 乱立（暫定: Levenshtein 距離 2 以下の slug が 3 件以上で warn）

具体閾値の調整は別 PR（設定 key 化）。

### `#<slug>` 本文中検出はスコープ外

本文中の `#<slug>` 参照の検出 / 補完は submit-segment 系チケットの責務。本チケットの Linter は frontmatter 由来の参照（`replaced_by` / `requires`）のみを検証する。

### 参照整合チェックの実行方式

write/edit 毎に `<workspace>/memory/`、`<workspace>/knowledge/` を毎回 walk して slug 集合を構築（キャッシュなし）。ファイル数は少ない想定。

### 適用経路

- memory tool の write/edit 内で pre-write 検証
- 人間編集 / pre-commit hook 経路は本チケットでは作らない。`memory::Linter` を pure 関数として export しておけば後で CLI 化できる

## 範囲外

- 検索ツール、常駐注入、Phase 1/2、GC の実装
- 意味破壊（rewrite で主張が落ちる等）の検出 — 監査 LLM 層は将来検討
- staging JSON の schema — Phase 1 チケット
- Workflow の `/<slug>` 実行経路
- 本文中 `#<slug>` 参照の検出 — submit-segment 系
- 設定 key（閾値 tune）— 別 PR
- 人間編集向け CLI / pre-commit hook — 別チケット
- `Interceptor` / `Hook` 系統への拡張 — 不要（tool error で完結）

## 完了条件

- `crates/memory/` が新設され、workspace に登録されている
- memory tool 3 種（read / write / edit）が登録できる
- write/edit に違反 content を渡すと、複数違反を集約した `ToolError::InvalidArgument` が返り、fs に書き込まれない
- 正常 content は通常通り書き込まれる
- `memory/workflow/<slug>.md` への write/edit は error で止まる
- 同 slug で新規作成しようとすると error になる（existing → edit に倒すサイン）
- `replaced_by` / `requires` の参照切れと循環が error として検出される
- Pod が memory を有効化すると、generic tool の Scope から memory/knowledge が deny される
- 既存ビルド・テストを壊さない

## 実装順序

1. `crates/memory/` 新設、workspace 登録、依存追加
2. `schema/`, `slug.rs`, `error.rs`（pure 関数 + 型）
3. `linter/`（frontmatter / size / 参照存在 / 循環 / workflow 拒否）
4. `tool/`（read / write / edit、pre-write で linter 通す）
5. Pod 側の Scope deny 配線
6. 単体テスト

各ステップ終了時点でビルド通過を維持する。

## 参照

- `docs/plan/memory.md` §ファイル形式 / §書き込み経路と Linter / §Knowledge の採択基準（本チケットで該当箇所を memory 専用 tool 方式に更新）
- `docs/plan/workflow.md` §格納先とファイル形式 / §生成・更新ポリシー
- `crates/tools/src/{write,edit,read}.rs` — Tool 実装の参考（依存はしない）
- `crates/llm-worker/src/tool.rs` — `Tool` trait / `ToolError` / `ToolOutput`

## Review

- 状態: Approve with follow-up
- レビュー詳細: [./memory-file-format.review.md](./memory-file-format.review.md)
- 日付: 2026-04-27
