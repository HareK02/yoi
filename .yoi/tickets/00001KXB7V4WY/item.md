---
title: 'Runtime restart-crossing Worker restore path'
state: 'closed'
created_at: '2026-07-12T13:21:38Z'
updated_at: '2026-07-13T12:05:05Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-12T14:16:14Z'
---

## 背景

Filesystem-backed Runtime は persisted Runtime / Worker record を reload できるが、Runtime process の再起動後は persisted connected execution mapping が stale 扱いになり、live Worker controller / handle は復元されない。そのため、復元後の Worker に input を送ると `worker has no execution backend` で失敗する。

TUI の restore 経路では、live row は socket override で attach し、stopped row は Worker name だけを使って Worker runtime command を spawn する。restore 本体は name-keyed metadata と session log replay を authority とし、`Worker::restore_from_worker_metadata_with_context` が active session/segment、manifest snapshot、session log、duplicate writer 防止 allocation、delegated child scope reconciliation を扱う。

Runtime の restart-crossing restore もこの境界に揃え、persisted execution binding を liveness と見なさず、実 controller / handle を明示的に materialize する経路を設計・実装する。

## 要件

- Runtime process restart 後、fs-backed Runtime が persisted active Worker を execution backend 経由で復元できる。
- Persisted execution binding は restore hint に留め、live handle の証明として扱わない。
- Restore authority は TUI と同じく deterministic Worker name metadata と session log replay に置く。
- Fresh Worker creation と persisted Worker restore は execution backend / factory API 上で分ける。
- Restore 後は fresh spawn と同じ protocol/event observation bridge を再接続する。
- Workdir は新規 materialize せず、persisted assignment / Workdir id を rebind する。
- 同じ Workdir が別の active Worker に primary assigned されている場合は restore を拒否または stale diagnostic にする。
- 個別 Worker の restore failure で Runtime startup 全体を失敗させない。対象 Worker は stale のまま diagnostic を残す。
- Pending / no-history Worker を明示的に扱い、initial input の二重投入を避ける。

## 設計メモ

- `WorkerExecutionOperation::Restore` を追加する。
- `WorkerExecutionRestoreRequest` と `WorkerExecutionBackend::restore_worker` を追加する。
- `RuntimeWorkerFactory::restore_controller` を追加し、`ProfileRuntimeWorkerFactory` では resolved active segment を持つ Worker に対して `Worker::restore_from_worker_metadata_with_context` を呼ぶ。
- Runtime は `RuntimeState::from_persisted` の stale marking を維持し、execution backend install 後に active stale candidates の restore pass を走らせる。
- Restore 成功時は `execution_handle`、connected binding/status、run state、Workdir status、snapshot、restore event を commit する専用経路を持つ。
- Restore 失敗時は `execution_handle = None` のまま、stale status と `worker_execution_restore_failed` diagnostic を残す。
- Pending metadata で resolved `segment_id` がない場合は、少なくとも no-history / no-initial-input Worker と initial-input 境界が曖昧な Worker を区別する。

## 受け入れ条件

- fs-backed Runtime の再起動後、valid metadata / session log を持つ active Worker が live execution handle 付きで restored connected になり、`send_input` が成功する。
- Worker metadata missing / corrupt / pending conflict などの restore failure は Runtime startup を止めず、該当 Worker を stale のまま clear diagnostic として表示する。
- Pending no-history Worker の restore behavior がテストされ、initial input を二重投入しない。
- Workdir restore は persisted Workdir binding を再利用し、新しい Workdir を作らない。
- Spawn と restore が同じ observation bridge behavior を共有し、frontend / WebSocket observation が restored Worker の状態変化を受け取れる。
