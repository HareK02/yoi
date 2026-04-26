# メモリ機構: memory / Knowledge 検索ツール

## 背景

`docs/plan/memory.md` §retrieval 経路 で定義した 2 本の検索ツールを Pod から呼べる LLM ツールとして実装する。memory 検索と Knowledge 検索は対象ディレクトリが違うだけで同型の仕様。grep ベースで始め、FTS / vector は将来検討。

このツールは Phase 2 Pod の agentic 探索経路としても、通常 Pod の `#<slug>` 展開経路としても、使用頻度メトリクスの観測点としても使う（メトリクスの hook 挿入は本チケットの範囲外、経路だけ揃える）。

## 要件

### ツール仕様（両者共通）

- Input:
  - `query: string`（自由文字列、必須）
  - `slug: string`（完全一致 1 件返し、`#<slug>` 解決に使う）
  - `kind: string`（Knowledge のみ、filter）
- Output: `{ slug, kind, description, model_invokation, excerpt }` の配列
  - `excerpt` はマッチ箇所の前後数行
- ソート: grep 出現順（初期）
- ヒット件数上限と excerpt 行数は設定ファイルで tune
- 対象ファイルは都度スキャン。派生 index は持たない

### 対象

- memory 検索: `memory/{summary,decisions,requests}/*.md`（workflow / \_staging は除外）
- Knowledge 検索: `knowledge/*.md`

### 登録

- 通常 Pod と Phase 2 Pod の両方に渡せる tool 定義
- Phase 2 Pod には Knowledge 検索を必ず渡す（全 Knowledge 本文を prompt に埋めない前提）

## 範囲外

- 使用頻度メトリクス本体（hook 点の予約のみ。カウント・レポートは別チケット）
- slug サジェスト補完 UI（TUI 側、別途）
- FTS / vector index
- 常駐注入（別チケット）

## 完了条件

- 通常 Pod / Phase 2 Pod 双方から両ツールが呼べる
- `slug` 指定で完全一致の 1 件が返り、`#<slug>` の本文展開経路として成立する
- `query` 指定で frontmatter 含む全文から excerpt 付きでヒットが返る
- Knowledge 検索の `kind` filter が効く
- 対象外ディレクトリ（`memory/workflow/`, `memory/_staging/`）はヒットしない

## 参照

- `docs/plan/memory.md` §retrieval 経路 / §Knowledge の採択基準
- `tickets/memory-file-format.md`（依存: frontmatter スキーマ）
