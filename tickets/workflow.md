# Workflow 実装

## 背景

`docs/plan/workflow.md` で決まった「制約付きの強制的な作業フロー」を `/<slug>` で呼び出せるようにする。Knowledge (`#<slug>`) を依存として inject できる経路を持つことで、procedural な能力を再利用可能な単位に固定する。

memory 機構（`docs/plan/memory.md`）からは独立してスタートできる: Workflow は人間が書く / consolidation の offer 経由でしか作られず、自動書き込み禁止のため Phase 2 の前提に依存しない。Knowledge resolver は `requires` の inject 経路として相互依存する。

agent-skills (agentskills.io 形式) は本チケットの ingest 経路を再利用して Workflow として読み込む側になる（`tickets/agent-skills.md` 参照）。

## 決定事項の参照

詳細は `docs/plan/workflow.md` 参照。要点のみ:

- 呼び出し: `/<slug>`、フラットな名前空間、kebab-case
- 配置: `<workspace_root>/.insomnia/memory/workflow/<slug>.md`（ファイル名 = slug、frontmatter に `name` を持たない）
- frontmatter: `description` / `model_invokation` (default OFF) / `user_invocable` (default ON) / `requires: [knowledge-slug, ...]`
- 実行: `requires` の Knowledge 本文を context に inject してから Workflow 本文を実行
- 自動書き込み禁止（consolidation の write tool schema に `workflow` カテゴリを含めないことで構造的に担保。Linter で人間にも見える形で再保証）

## 方針

### MVP スコープ

1. **Workflow loader / 検証**
   - `<workspace_root>/.insomnia/memory/workflow/*.md` を走査
   - frontmatter を仕様通り検証。必須欠落・型不一致・slug とファイル名の不一致は hard error（Pod 起動失敗）
   - 未知フィールドは `tracing::warn!` して無視（既存 manifest と同方針）
   - 重複 slug は最初に見つかったものを採用 + warn（後述の skill ingest が乗ると衝突解決ルールが追加で必要になる）

2. **`/<slug>` 呼び出し経路**
   - `Segment::WorkflowInvoke { slug }` を Pod 側で resolve
   - 解決失敗（slug 未登録 / `user_invocable: false`）は `ToolError` 相当でユーザーに返し、Worker には届かない
   - `requires` の Knowledge 本文を Knowledge 検索ツールの slug 完全一致経路で取得し、Workflow 本文の前に context へ inject
   - Workflow 本文は Markdown のままサブミット内容として扱う（DSL 化はしない）

3. **`model_invokation` 注入**
   - `model_invokation: true` な Workflow の `description` を通常 Pod の system prompt に常駐注入する。Phase 2 prompt には入れない
   - 予算は Knowledge の常駐注入（`memory.md` §retrieval 経路）と合算管理。description 上限は agentskills 準拠の 1024 chars に揃える

4. **Linter ルール**
   - `memory/workflow/*.md` への write/edit は memory 専用 Tool 経由でのみ許可（汎用 Write/Edit は Scope deny）
   - consolidation の write tool schema からは `workflow` カテゴリを除外（自動書き込み禁止の構造的担保）
   - Workflow 自体の Linter は frontmatter 検証 + slug/ファイル名一致のみ。意味検証は将来検討

### 範囲外

- DSL 化や step 粒度の制約（Markdown 本文をそのまま実行）
- 中断・再開・トランザクション管理
- 品質検証フロー（external author empirical-prompt-tuning 相当、`docs/plan/workflow.md` §将来検討）
- LLM による Workflow 自律生成（offer までで留める方針は本チケットでは扱わず、consolidation 側の責務）
- Knowledge 検索ツール本体の実装（memory チケット側）。本チケットは slug 完全一致経路の利用者に留まる

## 完了条件

- `<workspace_root>/.insomnia/memory/workflow/*.md` をロードし、frontmatter 違反は Pod 起動エラーになる
- `/<slug>` を含む submit が `Segment::WorkflowInvoke` として送られ、Pod 側で `requires` Knowledge を inject した上で本文が実行される
- `model_invokation: true` の Workflow description が通常 Pod の system prompt に列挙される
- `user_invocable: false` の Workflow は `/<slug>` 補完候補から除外され、明示呼び出しもエラーになる
- 単体テストで frontmatter 検証の正常 / 異常系、`requires` 解決、フラグ別の挙動が verify される

## 実装順序

1. `manifest` または既存 memory クレートに `Workflow` 構造体と `WorkflowDirectoryLoader` を置く。frontmatter パースと検証のみでテスト完結
2. Pod に Workflow registry を持たせ、`model_invokation` description の system prompt 注入を組む
3. `Segment::WorkflowInvoke` の resolver を Pod 側に実装。Knowledge 検索ツールの slug 完全一致経路で `requires` を inject
4. 汎用 Write/Edit に対する `memory/workflow/` deny を Scope に追加、Linter 仕上げ

各ステップ終了時点でビルド通過・既存テスト合格を維持する。

## 参照

- 設計: `docs/plan/workflow.md`
- Knowledge / `#<slug>` の retrieval: `docs/plan/memory.md` §retrieval 経路
- Submit segment: `tickets/submit-tui-completion.md`（`Atom::WorkflowInvoke`）、`tickets/session-log-segments.md`
- 後続: `tickets/agent-skills.md`（外部 SKILL を Workflow として ingest する経路）

## Review

- 状態: Approve
- レビュー詳細: [./workflow.review.md](./workflow.review.md)
- 日付: 2026-05-02
- 主要指摘: なし（フォローアップ／nit レベルのみ）。
