---
title: 'worker-runtimeにWebSocket event stream serverを追加する'
state: 'ready'
created_at: '2026-06-25T14:44:02Z'
updated_at: '2026-06-25T16:34:16Z'
assignee: null
---

## 背景

Runtime command は REST/HTTP でよいが、Worker output / status / transcript update を Backend が追うには observation transport が必要になる。Runtime から Backend へ能動接続する相互型は v0 では採用せず、Backend が Runtime の event stream に接続する形にする。Browser は Runtime event stream に直接接続せず、Backend が proxy / projection する。

この Ticket では `worker-runtime` process に WebSocket based observation server を追加する。SSE は将来追加してよいが、v0 の実装対象は Backend-owned WebSocket client が接続する Runtime event stream とする。command API とは分離する。

## 要件

- `worker-runtime` に `ws-server` feature を追加する。
- Feature disabled 時、core library は stream server dependency を強制しない。
- Runtime process が Worker / Runtime events を WebSocket observation endpoint で公開できる。
- Endpoint は少なくとも以下のどちらかを扱う。
  - `GET /v1/events/ws?cursor=...`
  - `GET /v1/workers/{worker_id}/events/ws?cursor=...`
- Event stream は Runtime lib の event bus / event log を元にする。
- Event cursor / event id は Runtime local opaque id とする。
- Reconnect / cursor resume / bounded backlog / unknown cursor の扱いを typed にする。
- Transcript projection polling endpoint と event stream の責務を分ける。
- Browser direct Runtime access は想定せず、Backend client が購読する。

## Non-goals

- REST command server implementation。
- Backend event proxy UI implementation。
- Runtime-initiated Backend push connection。
- Full exactly-once delivery。
- Browser-facing WebSocket protocol。

## 受け入れ条件

- `worker-runtime` に optional `ws-server` feature がある。
- Feature disabled でも `worker-runtime` core が compile できる。
- Runtime process exposes worker/runtime WebSocket event stream endpoint.
- Backend client can reconnect with cursor / last event id semantics at the protocol level.
- Unknown cursor / expired cursor / worker not found are typed errors or stream diagnostics.
- WebSocket event stream tests cover at least connect, event delivery, cursor resume, and worker-scoped filtering.
- `cargo test -p worker-runtime --features ws-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
