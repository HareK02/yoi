# 内部 Worker / 内部 Pod の Workflow 化

## 背景

INSOMNIA が内部で固定 prompt を持って disposable Worker / 専用 Pod を立ち上げている経路がいくつかある:

- extract 活動抽出（`crates/memory/src/extract/prompt.rs::EXTRACT_SYSTEM_PROMPT`）
- consolidation 統合 + 整理（`tickets/memory-consolidation.md`、本チケット時点では実装中 / 直前）
- Compact（`PromptCatalog::compact_system`）

これらは実装内 `&str` 定数や `PromptCatalog` の overlay で管理されており、prompt の調整や運用カスタマイズが「コード変更 + 再ビルド」を要する。一方、ユーザー向け `/<slug>` Workflow（`tickets/workflow.md`）は `<workspace_root>/.insomnia/workflow/<slug>.md` に住み、frontmatter + Markdown 本文 + `requires` Knowledge inject を持つ宣言形式で運用できる。

両者を寄せ、内部 Worker / 内部 Pod の prompt + ツール surface + Knowledge 依存を **Workflow と同一仕様で記述** できる経路を用意する。これにより:

- 内部 prompt の運用調整が workspace 側でできる（コード変更不要）
- consolidation の prompt 案 (`docs/plan/memory-prompts.md`) を workspace に直接 ingest できる
- 将来 consolidation を独立 Pod に引き上げる際も、Workflow を submit する形に揃えられる

## 要件

### Workflow の役割拡張

`tickets/workflow.md` の Workflow 仕様は「ユーザーが `/<slug>` で submit する制約付き作業」だが、本チケットでは **内部トリガー（Pod 内部の状態遷移）から呼び出される Workflow** を一級扱いに広げる。

- 同じファイル形式（`.insomnia/workflow/<slug>.md`）、同じ frontmatter / Linter
- `user_invocable: false` で `/<slug>` 経路から見えなくする
- `model_invokation` は通常 Pod 用の system prompt 注入仕様のまま（内部 Workflow は通常 OFF）
- 内部 Workflow を識別するキー（例: `internal_role`）と、必要なツール surface を表明する手段を frontmatter に追加する。具体 schema は実装で詰める

### 内部呼び出し経路

Pod 側の既存トリガー（extract post-run / consolidation staging 閾値 / Compact 閾値 等）は固定 `&str` の代わりに Workflow loader 経由で:

1. 内部識別キーで該当 Workflow を解決（衝突時は workspace 上書き優先、なければ insomnia bundled default）
2. `requires` Knowledge を本文の前に inject
3. Workflow 本文を sub-Worker / sub-Pod の prompt として渡す（system prompt 扱いか初回 submit 扱いかは内部用途で固定し、role ごとに揃える）
4. 既存のツール登録ロジックは Workflow が表明したツール surface に従う

### Bundled defaults

ユーザー workspace に該当 Workflow が無い場合に備え、insomnia 同梱の default Workflow を読む層を `PromptCatalog` の overlay と整合する形で持つ。

- 既存の Pod prompt 4 層 overlay（builtin / user / workspace / pod-pack）と同じ優先順
- bundled default の物理配置は実装で決める

### 関連チケットとの順序

- `tickets/workflow.md`（ユーザー向け Workflow 本体）が先行する。本チケットはその仕様を前提に「内部呼び出し経路」を追加する側
- `tickets/memory-consolidation.md` は当面 `&str` 定数で実装してよい。本チケット完了時に Workflow 化に乗り換える
- extract / Compact も同様に role ごとに段階移行

## 範囲外

- Workflow 仕様自体の本体実装（`tickets/workflow.md`）
- 内部 Workflow の自動生成（consolidation の offer 等。`docs/plan/memory.md` §Offer 経路 / 将来検討）
- 既存 `&str` 定数の物理削除タイミング（移行が完了した role ごとに削除する運用）
- `model_invokation` 注入予算の最適化（既存 Knowledge 常駐注入予算と合算する規約は `docs/plan/memory.md` 側）

## 完了条件

- 各内部 Worker / 内部 Pod（少なくとも extract / consolidation / Compact のうち、本チケット着手時点で実装済みのもの）が内部識別キー付き Workflow を解決して prompt とツール surface を組み立てる
- workspace で `.insomnia/workflow/<slug>.md` を上書きすれば内部 Worker の prompt が変わる
- workspace に該当 Workflow が無い場合、bundled default が使われる
- `user_invocable: false` の内部 Workflow は `/<slug>` 候補から除外され、ユーザーからは呼べない
- 内部 Workflow も consolidation の自動書き込み禁止対象のまま（Linter で構造的担保、`workflow.md` と整合）
- 単体テストで bundled default / workspace overlay / ツール surface 表明 + 解決 + 適用がカバーされる

## 参照

- 前提: `tickets/workflow.md`
- 最初の利用者: `tickets/memory-consolidation.md`
- 関連: `tickets/agent-skills.md`（外部 SKILL ingest 経路。本チケットの内部呼び出し経路とは別軸）
- 設計: `docs/plan/workflow.md`、`docs/plan/memory.md`、`docs/plan/memory-prompts.md`
