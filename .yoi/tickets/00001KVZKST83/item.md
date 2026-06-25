---
title: 'worker-runtimeにFS永続化featureを追加する'
state: 'queued'
created_at: '2026-06-25T14:44:02Z'
updated_at: '2026-06-25T16:39:26Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T16:39:26Z'
---

## 背景

`worker-runtime` core は embedded use のため memory store を持つが、独立 Runtime process では process restart に耐える local persistence が必要になる。Backend は SQLite control plane / projection を持つが、remote Runtime process が Backend SQLite を直接 authority とする設計にはしない。Runtime execution store は Runtime 側の local filesystem に置くのが v0 では最も単純で、既存 `pod-store` / session jsonl / runtime dir の知見も活用できる。

この Ticket では `worker-runtime` の `fs-store` feature と filesystem persistence backend を実装する。HTTP server / event stream server は別 Ticket とする。

## 要件

- `worker-runtime` に `fs-store` feature を追加する。
- Feature disabled 時、core library は FS store dependency を強制しない。
- `FsRuntimeStore` 相当を追加し、Runtime の store backend として選択できる。
- Filesystem layout は Runtime scoped / Worker scoped にする。
  - runtime record / config snapshot。
  - worker record。
  - worker state。
  - event log JSONL。
  - transcript projection JSONL。
  - overview / usage projection。
- Store は `runtime_id + worker_id` を authority とし、`pod_name` / socket path / legacy session path を authority にしない。
- Atomic write / directory creation / corrupt record diagnostics / bounded read を扱う。
- 旧 `pod-store` の metadata JSON / active segment pointer / atomic write pattern は参考にするが、standalone `worker-store` crate は作らない。
- Memory store tests と FS store tests の両方が通る。

## Non-goals

- REST command server。
- SSE / WebSocket event stream server。
- Backend RuntimeRegistry integration。
- Full legacy Pod session migration。
- SQLite Runtime store。
- Standalone `worker-store` crate。

## 受け入れ条件

- `worker-runtime` に optional `fs-store` feature がある。
- `fs-store` disabled でも `worker-runtime` core が compile できる。
- `FsRuntimeStore` が Runtime の persistence backend として使える。
- Worker create / state update / event append / transcript append / bounded read が FS store で動く。
- FS layout が Runtime/Worker scoped であり、legacy Pod path を public authority にしていない。
- Corrupt/missing files are surfaced as typed diagnostics/errors.
- `cargo test -p worker-runtime --no-default-features` が通る。
- `cargo test -p worker-runtime --features fs-store` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
