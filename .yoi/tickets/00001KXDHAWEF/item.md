---
title: 'Show backend operation error details'
state: 'done'
created_at: '2026-07-13T10:45:59Z'
updated_at: '2026-07-13T10:51:20Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-13T10:46:30Z'
---

## 背景

Workspace cleanup などの backend operation failure が `operation failed; backend-private details were omitted` に置き換えられ、実際の失敗理由が UI に出ない。現在は local workspace backend / trusted local UI の開発中であり、原因調査のためには backend error message をそのまま表示できる必要がある。

## 要件

- Workspace API error response の backend operation message を固定の omitted 文言に置換しない。
- `workspace_cleanup_plan_stale` などの RuntimeOperationFailed message が UI/API response にそのまま乗る。
- 既存の diagnostic surface は維持する。

## 受け入れ条件

- `sanitize_backend_error` が入力 message を保持する。
- 既存の omitted-path 前提 test を新しい方針に合わせる。
- workspace-server tests が通る。
