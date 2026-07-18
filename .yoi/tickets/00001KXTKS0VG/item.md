---
title: 'Preserve Workdirs when Workers stop or delete'
state: 'closed'
created_at: '2026-07-18T12:38:47Z'
updated_at: '2026-07-18T12:49:12Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-18T12:39:18Z'
---

## 背景

Backend-managed Workdir は Worker とは独立した再利用可能 resource として扱う。現在は Worker stop/delete 時に runtime が Worker の working directory binding を `materializer.cleanup` してしまい、workspace-server 側の Workdir record だけが残って `corrupted` と表示される。

Worker lifecycle と Workdir lifecycle を分離し、Worker の削除は Workdir の占有を解放するだけにする。Workdir 実体の削除は明示的な Workdir cleanup/delete API に限定する。

## 要件

- Worker stop/delete では Workdir 実体を削除しない。
- Worker spawn failure rollback では、その spawn request で新規 materialize した Workdir だけ cleanup する。
- 既存 Workdir に bind した spawn failure では Workdir を cleanup しない。
- Workdir cleanup API 経由の明示 cleanup は維持する。

## 受け入れ条件

- `stop_worker` 後も Worker に bind されていた Workdir 実体が残る。
- Worker spawn failure で既存 Workdir bind が使われた場合、Workdir 実体は残る。
- Worker spawn failure で新規 materialize した場合だけ rollback cleanup される。
- focused worker-runtime tests が通る。
