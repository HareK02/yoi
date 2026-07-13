---
title: 'Allow not-found Workdir cleanup action'
state: 'closed'
created_at: '2026-07-13T11:43:33Z'
updated_at: '2026-07-13T12:04:57Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-13T11:44:18Z'
---

## 背景

Workdir list では Runtime から `not_found` が確認できているが、cleanup plan では `file_status` が `notfound` になり、`workdir_record_delete` ではなく `workdir_dirty_discard` と判定される。そのため frontend の delete action が disabled になっている。

原因は `WorkingDirectoryStatusKind::NotFound` を `format!("{:?}").to_lowercase()` で string 化しているためで、serde の `snake_case` 表現 (`not_found`) とずれている。

## 要件

- Workdir cleanup plan の observed Runtime status string は Debug 表現ではなく canonical snake_case label を使う。
- `WorkingDirectoryStatusKind::NotFound` は `not_found` として扱われる。
- `not_found` Workdir candidate は `workdir_record_delete` action になり、blocking reason がなければ削除可能になる。

## 受け入れ条件

- cleanup plan API で not-found Workdir の `file_status` が `not_found` になる。
- cleanup plan API で not-found Workdir の `action` が `workdir_record_delete` になる。
- workspace-server tests が通る。
