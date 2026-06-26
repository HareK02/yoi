---
title: 'worker-runtimeにREST command serverを追加する'
state: 'done'
created_at: '2026-06-25T14:44:02Z'
updated_at: '2026-06-26T04:20:31Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T16:39:39Z'
---

## 背景

`worker-runtime` は library として Backend に組み込めるだけでなく、別ホスト上の独立 Runtime process としても動かしたい。独立 process の Runtime は、Backend が client として接続できる command API を公開する必要がある。Browser は Runtime process に直接接続せず、常に Backend を経由する。

この Ticket では observation stream ではなく、Worker 操作 command 用の REST/HTTP server と `worker-runtime/main.rs` の最小 process wrapper を実装する。

## 要件

- `worker-runtime` に `http-server` feature を追加する。
- `http-server` disabled 時、core library は HTTP server dependency を強制しない。
- `worker-runtime/main.rs` または binary entrypoint が Runtime を起動し、HTTP command API を公開する。
- Command API は少なくとも以下を扱う。
  - `GET /v1/runtime`
  - `GET /v1/workers`
  - `GET /v1/workers/{worker_id}`
  - `POST /v1/workers`
  - `POST /v1/workers/{worker_id}/input`
  - `POST /v1/workers/{worker_id}/stop`
  - `POST /v1/workers/{worker_id}/cancel`
  - `GET /v1/workers/{worker_id}/transcript`
- API は Runtime lib の methods を呼ぶ wrapper とし、Worker semantics を二重実装しない。
- Command response は typed JSON shape とする。
- Busy / unknown worker / invalid input / unsupported operation / runtime unavailable を typed error にする。
- Runtime process config は v0 で最小限でよいが、runtime id / bind address / store selection を扱えるようにする。
- v0 auth は minimal local token placeholder でもよいが、Browser に Runtime credential を渡さない前提を崩さない。

## Non-goals

- SSE / WebSocket event stream server。
- Backend HTTP client integration。
- Dynamic Runtime registration。
- Browser direct Runtime access。
- Full auth / permission model。
- FS store implementation beyond using existing `fs-store` if available。

## 受け入れ条件

- `worker-runtime` に optional `http-server` feature がある。
- `http-server` disabled でも `worker-runtime` core が compile できる。
- Runtime process binary starts and exposes REST command endpoints.
- REST handlers delegate to `Runtime` lib API rather than duplicating Worker semantics.
- Worker create / input / stop / cancel / detail / transcript endpoints have typed request/response/error shapes.
- Browser-facing docs/comments state that Browser must go through Backend, not Runtime directly.
- Focused HTTP handler tests are added.
- `cargo test -p worker-runtime --features http-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
