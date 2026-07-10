---
title: 'Embedded no-workdir Worker authority policy'
state: 'planning'
priority: 'P1'
created_at: '2026-07-10T20:52:43Z'
updated_at: '2026-07-10T21:32:47Z'
assignee: null
---

## 背景

`worker/new` では現在 workdir 選択が必須になっているが、embedded Runtime 上に Worker を作る場合は workdir 無しで案内専用の Worker を起動できるようにしたい。

現状の整理では、`worker-runtime` の `CreateWorkerRequest` / `WorkerRecord.request` には `working_directory_request` / `working_directory` を保持でき、execution status にも `working_directory` を載せられる。一方、実際の FS tools の向き先はまだ Worker process の `cwd` と `manifest.scope` に依存している。`Read` / `Write` / `Edit` は絶対パス必須なので相対パスが即 cwd に吸われる問題は限定的だが、`Glob` / `Grep` の path 省略は `ScopedFs.cwd()` を使い、`Bash` は cwd を初期位置にするだけで filesystem sandbox ではない。

embedded no-workdir Worker では、workspace root への cwd fallback や default scope によって意図せず repository filesystem authority を持たないようにする必要がある。

## 要件

- `worker/new` は、選択 Runtime が embedded の場合に限り workdir 未指定で Create できる。
- embedded + workdir ありの場合は、Runtime/Execution backend が materialized working directory binding を Worker の effective cwd / status に反映する。
- embedded + workdir なしの場合は、workspace root fallback を filesystem authority として扱わない。
- embedded + workdir なしの場合は manifest overlay 等で filesystem tools / Bash / write-capable tools を無効化し、案内・会話中心の read-only/guidance-only Worker として扱う。
- FS tools は ambient process cwd ではなく、Worker の explicit effective working directory / authority を基準にする方向へ整理する。
- Bash は cwd 変更だけでは sandbox にならないため、no-workdir embedded では少なくとも無効化される。

## 受け入れ条件

- embedded Runtime 選択時、UI/API 経由で workdir 未指定 Worker を作成できる。
- non-embedded または filesystem authority が必要な Runtime/launch path では、既存の workdir 必須制約または明示 authority 要求が維持される。
- embedded no-workdir Worker の manifest/effective tool surface から repository FS 操作と Bash が利用できないことをテストで確認できる。
- embedded workdir あり Worker では、working directory status が Worker summary/detail に投影され、tools の基準 cwd が binding に一致する。
- `Glob` / `Grep` の path 省略や `Bash` の cwd に関する挙動が、no-workdir embedded の権限漏れにならない。
- 変更後に `cargo test` と `nix build .#yoi` が通る。
