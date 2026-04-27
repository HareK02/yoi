# メモリ機構: `model_invokation: ON` の常駐注入

## 背景

`docs/plan/memory.md` §retrieval 経路 の「常駐注入」項目。`model_invokation: ON` な Knowledge record の description を通常 Pod の system prompt に載せ、モデルが必要と判断した時点で検索ツール経由で本文を引く形を成立させる。Phase 2 Pod には注入しない。

専用の auto-invoke 経路は用意しない。モデルが description を見て自発的に検索ツールを呼ぶ経路に一本化する。

## 要件

- Pod 起動時に `knowledge/*` を走査し、`model_invokation: ON` の record の description を system prompt に連結
- Phase 2 Pod では注入しない（consolidation は検索ツール経由で自律探索）
- 予算はシステムプロンプト全体予算に含める。`memory/summary.md` の 5k 枠とは別管理にしない
- 超過時の件数キャップ / 優先順位ルールは初期不要（description 1024 chars 上限で通常は収まる前提）。ON record 数が増えて問題になったら別チケットで対応

## 範囲外

- auto-invoke 用の別経路（採用しない）
- ON/OFF 切替の自動判定（初期は手動。将来検討）
- Workflow 側の `auto_invoke` 同等機能 — 仕様対称だが本チケットは Knowledge のみ

## 完了条件

- `model_invokation: true` の knowledge を置いた状態で通常 Pod を起動すると、system prompt に description が含まれる
- `model_invokation: false` のものは含まれない
- Phase 2 Pod では注入されない
- 既存の system prompt 構成（AGENTS.md / scope summary / skills 等）と共存する

## 参照

- `docs/plan/memory.md` §retrieval 経路 / §Knowledge の呼び出し制御
- `tickets/memory-file-format.md`（依存: `model_invokation` frontmatter）

## Review
- 状態: Approve
- レビュー詳細: [./memory-resident-injection.review.md](./memory-resident-injection.review.md)
- 日付: 2026-04-27
