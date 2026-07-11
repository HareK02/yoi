---
title: 'Embedded no-workdir Worker authority policy'
state: 'inprogress'
priority: 'P1'
created_at: '2026-07-10T20:52:43Z'
updated_at: '2026-07-11T00:12:24Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-10T22:00:07Z'
---

## 背景

`worker/new` では現在 workdir 選択が必須になっているが、embedded Runtime 上に Worker を作る場合は workdir 無しで案内専用の Worker を起動できるようにしたい。

現状の整理では、前提チケット `00001KX6Y2A9Q` で Worker の filesystem authority が `WorkerFilesystemAuthority::{None, Local}` に分離され、Worker 直下の作業ディレクトリ property は削除される。このチケットではその前提を使い、embedded Runtime の workdir 未指定 spawn を `WorkerFilesystemAuthority::None` として扱う。

embedded no-workdir Worker では、workspace root や default scope によって意図せず repository filesystem authority を持たないようにする必要がある。

## 要件

- `worker/new` は、選択 Runtime が embedded の場合に限り workdir 未指定で Create できる。
- embedded + workdir ありの場合は、Runtime/Execution backend が materialized working directory binding を `WorkerFilesystemAuthority::Local` と Worker status に反映する。
- embedded + workdir なしの場合は、`WorkerFilesystemAuthority::None` を指定し、workspace root fallback を filesystem authority として扱わない。
- embedded + workdir なしの場合は filesystem tools / Bash / write-capable tools を登録せず、案内・会話中心の read-only/guidance-only Worker として扱う。
- FS tools は ambient process directory ではなく、Worker の explicit filesystem authority を基準に登録される。
- Bash は filesystem sandbox ではないため、no-workdir embedded では登録しない。

## 受け入れ条件

- embedded Runtime 選択時、UI/API 経由で workdir 未指定 Worker を作成できる。
- non-embedded または filesystem authority が必要な Runtime/launch path では、既存の workdir 必須制約または明示 authority 要求が維持される。
- embedded no-workdir Worker は `WorkerFilesystemAuthority::None` で作成される。
- embedded no-workdir Worker の model-visible tool surface に filesystem tools と Bash が現れないことをテストで確認できる。
- embedded workdir あり Worker では、working directory status が Worker summary/detail に投影され、tools の working directory base が `WorkerFilesystemAuthority::Local` に一致する。
- `Glob` / `Grep` の path 省略や `Bash` の初期ディレクトリ指定が、no-workdir embedded の権限漏れにならない。
- 変更後に `cargo test` と `nix build .#yoi` が通る。
