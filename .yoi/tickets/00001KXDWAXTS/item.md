---
title: 'Stop Worker before cleanup deletion'
state: 'planning'
created_at: '2026-07-13T13:58:15Z'
updated_at: '2026-07-13T13:58:45Z'
assignee: null
---

## 背景

Worker cleanup execution で selected Worker を削除すると、Runtime 側では Worker status がまだ active/running のため `delete_worker` が rejected し、workspace-server は `workspace_cleanup_worker_runtime_delete_rejected: Runtime did not delete selected Worker` を返す。

Cleanup の Worker delete action は、対象が cleanup plan で許可された場合、Runtime record を削除する前に Runtime 側 Worker を stopped に遷移させてから delete する必要がある。

## 要件

- Worker cleanup execution は Runtime delete の前に `stop_worker` を実行する。
- stop が rejected/error の場合は typed diagnostic を返して registry record を削除しない。
- stop 後の delete が成功した場合だけ backend registry row を削除する。
- 既に stopped / missing など delete 可能なケースを壊さない。

## 受け入れ条件

- Runtime が active Worker の delete を直接 reject するケースでも cleanup execution が stop -> delete の順に実行できる。
- workspace-server tests が通る。
