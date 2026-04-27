# メモリ機構: memory / Knowledge 検索ツール

## 背景

`docs/plan/memory.md` §retrieval 経路 の 2 本の検索ツールを Pod から呼べる LLM ツールとして実装する。memory 検索と Knowledge 検索は対象ディレクトリが違うだけ。grep ベースで始め、FTS / vector は将来検討。

このツールは Phase 2 Pod の agentic 探索経路としても、通常 Pod の `#<slug>` 展開経路としても、使用頻度メトリクスの観測点としても使う（メトリクスの hook 挿入は本チケットの範囲外、経路だけ揃える）。

slug 完全一致 1 件返しは検索ツールではなく `MemoryRead`（kind+slug 入力）が担当する。検索 → slug 入手 → Read で本文展開、という二段経路に整理する。

## 要件

### ツール構成

- **MemorySearch**: memory 配下の検索専用 Tool
- **KnowledgeSearch**: knowledge 配下の検索専用 Tool
- 単一ファイル取得（slug 完全一致）は `MemoryRead`（既存、本チケットで `kind+slug` 入力に切り替え）

### `MemoryRead` の入力変更（memory-file-format からの補正）

- 旧: `file_path`（絶対パス）
- 新: `kind: summary | decision | request | knowledge` + `slug`（summary は slug なし）
- 同じ補正を `MemoryWrite` / `MemoryEdit` にも適用し、Search 出力をそのまま投げ込める一貫した経路にする

### MemorySearch ツール仕様

- Input:
  - `query: string`（自由文字列、必須、case-insensitive 部分一致）
- Output: `{ slug, kind, excerpt }[]`
  - `kind` は record kind（`summary` / `decision` / `request`）
  - summary は slug を `"summary"` 固定で返す
  - `excerpt` はマッチ行の前後 N 行
- 対象: `memory/summary.md`, `memory/decisions/*.md`, `memory/requests/*.md`
- 除外: `memory/workflow/`, `memory/_staging/`（読みもしない）

### KnowledgeSearch ツール仕様

- Input:
  - `query: string`（必須）
  - `kind: string`（任意、KnowledgeFrontmatter の `kind` フィールドで filter）
- Output: `{ slug, kind, description, model_invokation, excerpt }[]`
  - `kind` / `description` / `model_invokation` は frontmatter から
  - frontmatter parse 失敗時は `null`（本文 grep は機能継続）
- 対象: `knowledge/*.md`

### 共通

- ソート: ファイル名昇順 → ファイル内行順（grep 出現順）
- ヒット件数上限と excerpt 行数は manifest `[memory]` で tune（`search_hit_limit` / `search_excerpt_lines`、デフォルト 20 / 前後 3 行）
- 対象ファイルは都度スキャン、派生 index は持たない
- frontmatter も検索対象に含める

### 登録

- 通常 Pod と Phase 2 Pod の両方に渡せる tool 定義
- Phase 2 Pod には Knowledge 検索を必ず渡す（全 Knowledge 本文を prompt に埋めない前提）
- 本チケットでは通常 Pod の controller への登録まで実装。Phase 2 Pod 側の配布は別チケット

## 範囲外

- 使用頻度メトリクス本体（hook 点の予約のみ。カウント・レポートは別チケット）
- slug サジェスト補完 UI（TUI 側、別途）
- FTS / vector index
- 常駐注入（別チケット）
- Phase 2 Pod 側のツール配布（Phase 2 チケットの責務）

## 完了条件

- 通常 Pod から MemorySearch / KnowledgeSearch / MemoryRead / MemoryWrite / MemoryEdit すべて呼べる
- Search が返す `slug` + `kind` をそのまま `MemoryRead` に渡して本文取得が成立する（`#<slug>` 展開経路）
- `query` 指定で frontmatter 含む全文から excerpt 付きでヒットが返る
- KnowledgeSearch の `kind` filter（frontmatter.kind 完全一致）が効く
- 対象外ディレクトリ（`memory/workflow/`, `memory/_staging/`）はヒットしない
- `search_hit_limit` / `search_excerpt_lines` の tuning が効く

## 参照

- `docs/plan/memory.md` §retrieval 経路 / §Knowledge の採択基準
- `tickets/memory-file-format.md`（依存: frontmatter スキーマ、本チケットで Read/Write/Edit 入力 schema を補正）

## Review
- 状態: Approve
- レビュー詳細: [./memory-search-tools.review.md](./memory-search-tools.review.md)
- 日付: 2026-04-27
