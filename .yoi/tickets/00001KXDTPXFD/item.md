---
title: 'Stabilize cleanup plan ordering'
state: 'closed'
created_at: '2026-07-13T13:29:51Z'
updated_at: '2026-07-15T16:19:29Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-13T13:30:39Z'
---

## 背景

Workdir cleanup endpoint を実際に叩くと、直前に取得した cleanup plan の revision / digest を送っても `workspace_cleanup_plan_stale` になる。cleanup plan の候補順序が HashMap / DB / merge の順序に依存しており、GET と POST 内部再生成で同じ集合でも digest が変わっている可能性が高い。

## 要件

- Cleanup plan の worker / workdir candidates を digest 計算前に deterministic に並べる。
- API response と execute 時再生成の plan digest が同一集合で安定する。
- not_found Workdir の `workdir_record_delete` が削除 endpoint で実行できる。

## 受け入れ条件

- cleanup plan を連続生成しても digest が安定する。
- 直前 plan の revision/digest を使った cleanup execution が stale 扱いにならない。
- workspace-server tests が通る。
