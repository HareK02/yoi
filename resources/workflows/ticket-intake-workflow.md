---
description: ユーザーの曖昧な依頼を要件同期し、合意済みの Ticket として作成・更新する Intake workflow
model_invokation: true
user_invocable: true
requires: [workflow-resource-boundary]
---

# Ticket intake workflow

1. ユーザー依頼と既存 Ticket を同期し、重複作成を避ける。既存 Ticket を対象にする場合は body/thread/artifacts を読んでから更新する。
2. 要件・背景・受け入れ条件・未決事項を Ticket に記録する。実装手順は必要になるまで増やしすぎない。
3. Ticket が queue 可能な粒度と明確さになったら、typed Ticket tool surface で intake summary を残し、`state = ready` にする。未決事項がある場合は planning に留め、必要な質問やリスクを明示する。
4. Handoff report は `created_or_updated_ticket_id`、`state`、`open_questions_or_risk_flags`、`intake_summary` を含める。
5. Intake は実装を開始しない。ユーザーが panel 等で `ready -> queued` し、Orchestrator が queued Ticket を routing する。
