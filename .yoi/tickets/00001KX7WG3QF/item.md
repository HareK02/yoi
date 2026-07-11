---
title: 'Remove duplicate Worker status field'
state: 'inprogress'
created_at: '2026-07-11T06:05:38Z'
updated_at: '2026-07-11T06:06:13Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-11T06:06:13Z'
---

## 背景

Workspace Backend/Frontend の Worker projection は `state` と `status` を両方持っているが、現状ほぼ同じ値を入れており区別がない。live/archived Worker 表示で不整合を避けるため、browser-facing Worker summary は `state` に一本化する。

## 要件

- Workspace browser-facing `WorkerSummary` から duplicate な `status` field を削除する。
- Backend projection は `state` のみを返す。
- Frontend types / UI / tests は `state` のみを参照する。
- Runtime 内部や別ドメインの typed status は必要ならそのまま残すが、Workspace browser Worker summary に duplicate status を露出しない。

## 受け入れ条件

- `/api/workers` の Worker item に `status` field が含まれない。
- `/api/runtimes/{runtime}/workers/{worker}` の Worker detail に `status` field が含まれない。
- Workspace frontend に `.status` 参照が残らない。
- `cargo test`、`cd web/workspace && deno task check`、`cd web/workspace && deno task test`、`nix build .#yoi` が通る。
