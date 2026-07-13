---
title: 'Preserve typed Workdir not-found errors'
state: 'closed'
created_at: '2026-07-13T10:58:32Z'
updated_at: '2026-07-13T12:05:01Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-13T10:59:02Z'
---

## 背景

Workdir detail / sync で Runtime 側に存在しない Workdir を問い合わせた時、worker-runtime は `WorkingDirectoryDiagnostic { code: working_directory_not_found }` を一度作っているにもかかわらず、RuntimeError::InvalidRequest に変換して REST error code が `invalid_request` になっている。workspace-server 側は diagnostic code の substring match で not_found を推測しており、typed error が失われて `unknown` になる。

## 要件

- Worker Runtime REST error response に Workdir diagnostic code を preserve する。
- workspace-server は diagnostic message / substring ではなく typed diagnostic code の exact match で Workdir not-found を扱う。
- Runtime に存在しない Workdir は `unknown` ではなく `not_found` / NotFound として扱える。

## 受け入れ条件

- `working_directory_not_found` が REST response の error.code として返る。
- workspace-server の `workdir_status_from_runtime_miss` が exact code match を使う。
- worker-runtime / workspace-server の関連テストが通る。
