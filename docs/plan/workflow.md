# Workflow の方針

## Context

Workflow は制約付きの強制的な作業フロー。`/<slug>` で明示的に呼び出し、依存 Knowledge を context に inject してから実行する。Knowledge（`#<slug>`）は `docs/plan/memory.md` 側で定義。

## 決定事項

### 呼び出しと依存

- 呼び出し: `/<slug>`
- 名前空間はフラット、slug は kebab-case（小文字英数とハイフン）
- frontmatter `requires: [knowledge-slug, ...]` で依存 Knowledge を slug 参照
- 実行時は依存 Knowledge 本文を context に inject してから Workflow 本文を実行

### 呼び出し制御フラグ

| フラグ           | 意味                                                    | デフォルト |
| ---------------- | ------------------------------------------------------- | ---------- |
| `auto_invoke`    | description が LLM context に載り、LLM が自発的に呼べる | **OFF**    |
| `user_invocable` | ユーザーが `/<slug>` で明示的に呼べる                   | **ON**     |

`auto_invoke` の ON 化は人間の判断、または consolidation からの offer 経由のみ。同じ制御は Knowledge 側（`memory.md`）でも採用。

### 格納先とファイル形式

- `.yoi/workflow/<slug>.md`（ファイル名 = slug がそのまま識別子、`name` field は持たない）
- `.yoi/memory/` は session-derived state 専用、Workflow は配置しない
- frontmatter + Markdown 本文
- frontmatter フィールド: `description`, `auto_invoke`, `user_invocable`, `requires`

### 生成・更新ポリシー

Workflow は**人間が書く**、または consolidation が offer して人間が承認する。自動書き込みは禁止:

- consolidation（`memory.md` 参照）の write tool schema に `workflow` カテゴリを含めないことで構造的に担保
- 新規作成 / 手順追加・更新は `Event::Notification` で提案し、人間承認で反映

### Offer 契機

consolidation が以下を検出した場合、Client に Notification を投げる:

- 再利用価値ある手続きの Workflow 化（新規作成）
- 既存 Workflow への改善提案（手順追加・更新）
- `auto_invoke` ON 化（頻繁に `user_invoke` されているものを検出）

## Scope 外

### 恒久除外（本設計方針として採用しない）

- Workflow の自律生成（offer までで留める。LLM が勝手に新規 Workflow を生成する経路は設けない）

### 将来検討（運用で必要性が見えたら追加）

- DSL 化や step 粒度の制約 — 初期は Markdown 本文そのまま実行
- Workflow 実行中の中断・再開・トランザクション管理
- 品質検証フロー: empirical prompt tuning pattern（`docs/ref/memory-systems.md` §6）相当の**新規 subagent 試走 + 構造化報告**を Workflow に適用。判定対象は本文の不明瞭点・裁量補完・要件達成率。Knowledge 単体の検証は設けず、`requires` 経由で Workflow から使われる前提で間接回収。SKILL 的用途（Workflow 経由しない `#knowledge`）は人間レビューに委ねる

## 関連

- `memory.md`: Knowledge 定義、extract/consolidation、Offer の配送経路
