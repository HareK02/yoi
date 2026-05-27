---
id: 20260527-000010-prompt-eval-metrics
slug: prompt-eval-metrics
title: Prompt / Workflow 評価メトリクスと改善 Offer
status: open
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:10Z
updated_at: 2026-05-27T00:00:10Z
assignee: null
legacy_ticket: tickets/prompt-eval-metrics.md
---

## Migration reference

- legacy_ticket: tickets/prompt-eval-metrics.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# Prompt / Workflow 評価メトリクスと改善 Offer

## 背景

empirical prompt tuning pattern は、agent-facing な指示（Skill / slash command / prompt 等）を新規 subagent に実行させ、実行者の自己申告と指示側メトリクスを突き合わせて反復改善する手法である。insomnia では Workflow / Skill ingest / Knowledge / memory consolidation / usage metrics / Pod orchestration があるため、この手法を単なる「手順」ではなく、**agent-facing instruction の品質観測 pipeline** として扱える。

特に insomnia では以下をシステム側で観測できる。

- evaluator Pod の session id / history
- tool call / tool result
- usage tokens
- workflow / knowledge の明示使用ログ（use 回数、last used、source breakdown。`tickets/memory-usage-metrics.md`）
- `model_invokation` 常駐注入の exposure cost 指標
- extract / consolidation による recurring pattern 抽出
- Workflow 自動書き込み禁止に基づく improvement offer

したがって、`/empirical-prompt-tuning` 相当の Workflow は、評価実行を orchestration するだけでなく、評価結果を構造化 event として残し、将来的に memory consolidation / usage metrics / Workflow improvement offer / `model_invokation` 判断へ接続するべきである。

## 要件

### `/empirical-prompt-tuning` Workflow

`.insomnia/workflow/empirical-prompt-tuning.md` を追加し、Workflow / Skill / prompt / Knowledge を評価対象として扱える手順を用意する。

Workflow は少なくとも以下を明示する。

- 評価対象 target の固定
  - kind: workflow / skill / prompt / knowledge
  - slug または path
  - git revision または content hash
- Iteration 0: description / body consistency check
- scenario set の作成
  - median 1 件
  - edge 1〜2 件
  - requirements checklist 3〜7 項目
  - `[critical]` 項目を最低 1 つ含める
- evaluator Pod は毎回新規に spawn し、同じ evaluator を再利用しない
- evaluator Pod は実装者ではなく評価者として動く
- evaluator report は以下の構造にする
  - Deliverable
  - Requirement achievement
  - Trace: Understanding / Planning / Execution / Formatting
  - Unclear points: Issue / Cause / General Fix Rule
  - Discretionary fill-ins
  - Retries
- 1 iteration 1 theme の最小修正を原則とする
- Workflow / prompt の実ファイル書き換えは人間承認後に限る
- Workflow 自動生成 / 自動更新は禁止し、必要な改善は offer として人間に戻す

### 評価 event schema

評価結果を、将来の system metrics / memory consolidation に流せる構造化 event として定義する。

最低限の field:

```text
eval_run_id
target_kind
target_slug_or_path
target_revision_or_hash
scenario_id
scenario_kind: median | edge | holdout
evaluator_pod_name
evaluator_session_id
started_at / ended_at
success: bool
accuracy: number
critical_passed: bool
tool_call_count
tool_call_count_by_tool
input_tokens
output_tokens
cache_read_tokens / cache_write_tokens if available
scope_error_count
file_search_count
escalation_count
unclear_points[]
  phase: Understanding | Planning | Execution | Formatting
  issue
  cause
  general_fix_rule
discretionary_fill_ins[]
retries
```

初期実装で全 field が機械取得できない場合は、取得可能なものと evaluator self-report 由来のものを分ける。未取得 field は空にしてよいが、schema 上は将来埋められる形にする。

### Metrics / memory consolidation との接続

本チケットでは、評価 event を memory / metrics pipeline に接続する設計を明文化し、可能な最小実装を入れる。

接続方針:

- evaluator self-report は consolidation extract の活動抽出対象になる
- repeated `General Fix Rule` は consolidation が recurring failure pattern として統合できる
- recurring pattern は即 Knowledge 化せず、明示使用ログと Doctor / prompt-eval の事後評価を通す
- Workflow 改善は `.insomnia/workflow/*.md` へ自動書き込みせず、Notification / report / ticket などの offer に留める
- `model_invokation` ON 判断では、明示使用ログと resident exposure cost に加えて、eval success rate / unclear point count / description-body consistency を判断材料にする

### 評価指標の解釈

Claude Code 版の `tool_uses` を、insomnia では tool 種別ごとの偏りとして解釈する。

例:

- Glob / Grep が突出: references / 探索方針が prompt 内で弱い
- Read が突出: required context の入口が弱い
- scope error が出る: permission / worktree / escalation 境界が弱い
- SpawnPod / SendToPod が多い: orchestration の粒度や子 Pod 指示が曖昧
- ticket / git write に向かう: escalation criteria が弱い

定量指標は補助であり、Unclear points / Discretionary fill-ins / General Fix Rule を主指標とする。

### Failure pattern ledger

手書き台帳だけにせず、eval event から抽出可能な failure pattern として扱う。

- `General Fix Rule` を class-level pattern として正規化する
- 同じ pattern が複数 scenario / 複数 iteration / 複数 target で再発した場合、consolidation が decision / knowledge candidate / workflow improvement offer に統合できる
- 同じ pattern が 3 回以上再発した場合、局所 patch ではなく target prompt の構造変更を提案する

## 範囲外

- Workflow の自動書き換え
- Knowledge の即時自動作成
- `model_invokation` ON/OFF の完全自動切替
- evaluator Pod の永続ジョブキュー化
- prompt DSL 化
- LLM judge による主観的 A/B 比較の採用
- すべての metrics field の初期実装での完全自動取得

## 完了条件

- `.insomnia/workflow/empirical-prompt-tuning.md` が追加され、insomnia の evaluator Pod / metrics / memory consolidation 前提で記述されている
- Workflow は Iteration 0、scenario checklist、Trace、Issue / Cause / General Fix Rule、1 iteration 1 theme、人間承認 gate を明示している
- 評価 event schema が docs または ticket 内で定義されている
- eval event を memory consolidation / usage metrics / Workflow improvement offer / `model_invokation` 判断へ接続する方針が文書化されている
- 既存の Workflow 自動生成禁止・history に commit されない context input 禁止・memory consolidation 方針に反していない
- `/auto-maintain` または `/worktree-workflow` のどちらか 1 件を対象に、構造審査または小規模 evaluator Pod 試走を行い、結果を記録している

## 参照

- empirical prompt tuning skill example（外部参照。取り込み時は必要最小限に一般化する）
- `docs/plan/workflow.md`
- `docs/plan/memory.md`
- `tickets/memory-usage-metrics.md`
- `tickets/auto-maintain-workflow.md`
