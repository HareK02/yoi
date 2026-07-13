---
title: 'Restore Worker without backend profile fetch'
state: 'planning'
created_at: '2026-07-13T14:44:12Z'
updated_at: '2026-07-13T14:44:40Z'
assignee: null
---

## 背景

Worker restore は Worker metadata の `resolved_manifest_snapshot` と session log を authority として復元できる。現状の worker-runtime restore path は fresh spawn と同じ profile source archive resolution を先に実行しており、Runtime restart 時に workspace-server / backend resource endpoint が未起動だと profile archive fetch に失敗して Worker が stale のまま残る。

Active Worker restore では backend profile source fetch を不要にし、metadata snapshot を使って復元する。

## 要件

- `ProfileRuntimeWorkerFactory::restore_controller` は active metadata restore の前に `request.profile_source` を解決しない。
- Restore は metadata の resolved manifest snapshot と builtins loader を authority にする。
- Pending/no-history fallback の fresh recreation が必要な場合だけ、従来通り profile source resolution を行う。
- backend が未起動でも、metadata snapshot を持つ active Worker restore は profile fetch failure で stale にならない。

## 受け入れ条件

- restore path の active case で `resolve_profile_source_archive` が呼ばれないことをテストする。
- worker-runtime tests が通る。
- Nix build が通る。
