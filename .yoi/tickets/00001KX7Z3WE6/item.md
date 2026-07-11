---
title: 'Disable Console links for archived Workers'
state: 'closed'
created_at: '2026-07-11T06:51:23Z'
updated_at: '2026-07-11T07:01:41Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-11T06:52:17Z'
---

## 背景

Workers list は live Worker と registry-only archived Worker を含む。Archived Worker は live Runtime Worker ではなく Console/detail API が 404 になるため、Sidebar/Workers page で Console に遷移可能な UI として表示してはいけない。

## 要件

- `implementation.kind == "backend_worker_registry"` または `state == "archived"` の Worker は Console link を表示しない。
- Sidebar では archived Worker を disabled/readonly 行として表示し、クリックできる Console target にしない。
- Workers page でも archived Worker の `Open Console` link を表示しない。
- live Worker の Console link は維持する。

## 受け入れ条件

- Archived Worker が Sidebar から Console にアクセス可能に見えない。
- Archived Worker が Workers page から Console にアクセス可能に見えない。
- `cd web/workspace && deno task check` と `cd web/workspace && deno task test` が通る。
