---
id: 20260527-000001-auto-maintain-workflow
slug: auto-maintain-workflow
title: 半自動開発運用 Workflow
status: closed
kind: task
priority: P2
labels: [migrated]
created_at: 2026-05-27T00:00:01Z
updated_at: 2026-06-05T15:56:29Z
assignee: null
legacy_ticket: tickets/auto-maintain-workflow.md
---

## Migration reference

- legacy_ticket: tickets/auto-maintain-workflow.md
- migrated_from: TODO.md / tickets directory migration on 2026-05-27

# 半自動開発運用 Workflow

## 背景

insomnia では insomnia 自身の開発を、ユーザーがタスクを投げ、設計相談をし、実装 Pod / reviewer Pod に分担させる形で進めている。既に Workflow / Skills、SpawnPod、Pod 間通信、scope 委譲、ticket / review lifecycle は揃っており、局所的な実装判断は AI に移譲できる余地が大きい。

一方で、完全な unattended 自動開発にするには、永続ジョブキュー、git 書き込み権限、設計判断のエスカレーション基準など未整理の領域がある。初期段階では、常駐 scheduler ではなく、ユーザーが明示的に起動する「maintainer workflow」として、TODO / tickets を俯瞰し、実装・レビュー・修正依頼を orchestration し、設計境界や完了判断だけを人間に戻す運用を整備する。

## 要件

### Workflow の役割

`/auto-maintain` 相当の Workflow を用意し、親 Pod が以下を実行できるようにする。

- `TODO.md` と `tickets/` から着手候補を把握する
- 既存方針・既存 ticket から実装方針が十分に導ける作業を選ぶ
- 要件が曖昧、または設計判断が必要な場合は実装前に人間へ質問する
- 実装 Pod を spawn し、適切な read / write scope を委譲する
- 実装 Pod の完了報告と diff / build / test 結果を確認する
- 必要に応じて reviewer Pod、または親 Pod 自身でレビューする
- レビュー指摘があれば修正を依頼する
- 最終的に「完了候補」として人間に報告する

### エスカレーション基準

Workflow は、少なくとも以下の場合に作業を止めて人間へ確認する。

- ticket の要件から複数の設計方針が自然に導け、選択が将来の構造に影響する
- scope / permission / history 永続化 / prompt context 加工原則など、システムの安全モデルに触れる
- 新しい ticket の追加、既存 ticket の大幅な要件変更、ticket 完了削除を行う
- git の commit / merge / push など書き込み操作が必要になる
- テスト不能、再現不能、または作業範囲外の不具合に遭遇する

### Pod orchestration 規約

- 実装 Pod と reviewer Pod は原則分ける。ただし scope 衝突や作業粒度により、親 Pod がレビューしてもよい。
- 実装 Pod に worktree write scope を渡す場合、review artifact を親または reviewer が書く前に実装 Pod を停止して scope を回収する。
- spawn 時は、作業対象 worktree の write scope だけでなく、必要な参照元 ticket / project root の read scope も明示する。
- 子 Pod の出力は `ReadPodOutput` で確認し、必要なら `SendToPod` で追加依頼する。
- orphan 化した Pod や不要になった Pod は `StopPod` する。

### 成果物

- Workflow 本文、またはそれに準じる運用手順が workspace から呼び出せる形で追加される
- Workflow が resident workflow として広告可能かどうかを判断し、必要なら `model_invokation` 設定を含める
- 実際の insomnia 開発 ticket を 1 件以上試走し、実装 Pod / review / 人間確認の境界が機能することを確認する
- 試走で見つかった不足（永続ジョブキュー、scope handoff、review artifact の置き場所等）は、本チケット内で解決せず、必要なら別 ticket として切り出す

## 範囲外

- 常駐 scheduler / daemon による unattended 実行
- git commit / merge / push の自動化
- ticket 完了削除の自動化
- Workflow の状態機械化、永続ジョブキュー化、トランザクション管理
- scope owner handoff など、Pod 権限モデル自体の変更

## 完了条件

- `/auto-maintain` 相当の半自動開発運用 Workflow が利用可能になっている
- Workflow は TODO / tickets から作業を選び、実装 Pod / reviewer / 人間確認を使い分ける手順を明示している
- エスカレーション基準により、設計判断・git 書き込み・ticket 完了判断が人間に戻る
- 少なくとも 1 件の小さな実開発作業で試走し、結果と不足点が記録されている
- 既存の Workflow / Skill / memory の設計方針、特に Workflow 自動生成禁止と history に commit されない context input 禁止に反していない

## 参照

- `docs/plan/workflow.md`
- `docs/report/2026-05-05-file-ticket-scope.md`
- `tickets/internal-worker-workflow.md`
