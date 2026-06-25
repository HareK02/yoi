---
title: 'worker-runtimeにWebSocket event stream serverを追加する'
state: 'ready'
created_at: '2026-06-25T14:44:02Z'
updated_at: '2026-06-25T20:10:54Z'
assignee: null
---

## 背景

Runtime command は REST/HTTP でよいが、Worker output / status / transcript update を Backend が追うには observation transport が必要になる。Runtime から Backend へ能動接続する相互型は v0 では採用せず、Backend が Runtime の event stream に接続する形にする。Browser は Runtime event stream に直接接続せず、Backend が proxy / projection する。

この Ticket では `worker-runtime` process に WebSocket based observation server を追加する。SSE は将来追加してよいが、v0 の実装対象は Backend-owned WebSocket client が接続する Runtime event stream とする。command API とは分離する。

## 目的

- Backend が Runtime 内 Worker の output / status / transcript update を購読できる。
- Runtime WS は新しい Worker output model を作らず、既存 `crates/protocol` の `protocol::Event` を observation payload として流す。
- Runtime WS は command channel ではなく、Backend-facing observation channel とする。
- Browser / Web UI へは Backend が proxy / projection し、Runtime endpoint / token / socket path / session path を直接渡さない。
- 後続の Web 権限制御で、Web origin からの操作や観測を Backend 側で block / redact / project できる境界を明示する。

## 要件

### Feature / server boundary

- `worker-runtime` に `ws-server` feature を追加する。
- Feature disabled 時、core library は stream server dependency を強制しない。
- Runtime process が Worker observation event を WebSocket endpoint で公開できる。
- Runtime WS endpoint は Backend-internal API として扱い、Browser-facing protocol ではない。
- Runtime WS は user input / command / mutation を受け付けない。
- Runtime WS 上に `protocol::Method` tunnel や arbitrary operation frame を作らない。

### Endpoint shape

- v0 required endpoint は worker-scoped stream とする。
  - `GET /v1/workers/{worker_id}/events/ws?cursor=...`
- Runtime-wide stream は v0 non-goal とする。ただし型や event id 設計は将来の runtime-wide stream を妨げない。
- Connect 時に対象 Worker の initial projection として `protocol::Event::Snapshot` 相当を最初に送る。
- Snapshot 後の live update は Runtime / Worker の broadcast event から生成される `protocol::Event` payload を送る。

### Wire envelope / protocol payload

- WebSocket frame は Runtime-local envelope を持ち、payload として `protocol::Event` を含める。
- Envelope は少なくとも以下を持つ。
  - opaque event id / cursor。
  - worker id。
  - `protocol::Event` payload。
  - optional diagnostic / stream control kind。
- `protocol::Event` は `crates/protocol` を authority とし、Runtime WS 専用の並行 event model を作らない。
- Runtime WS は `protocol::Event` の variant allowlist / subset を定義しない。Worker event bus が publish する `protocol::Event` payload を Runtime observation stream として forward する。
- Runtime WS が Web 表示可否を判断しない。Browser / Web UI 向けの filter / redact / projection は Backend projection layer の責務とする。
- Runtime WS の除外境界は variant subset ではなく channel / authority 境界とする。
  - `protocol::Method` tunnel を作らない。
  - user input / command / mutation frame を受け付けない。
  - raw provider trace / raw full session log / local filesystem paths / runtime credentials を authority payload として送らない。
- Method-specific reply を Runtime WS が passive event として合成しない。Worker event bus に publish された `protocol::Event` を forward する。

### Cursor / backlog / ordering

- Event id / cursor は Runtime local opaque id とする。
- Cursor は「最後に受け取った event id」を表し、resume 時はそれより後の event を送る。
- Delivery semantics は at-least-once とし、duplicate はあり得る。Backend client は id で de-dup できる。
- Ordering は worker-scoped stream 内で保持する。
- Bounded per-worker backlog を持ち、unknown cursor / expired cursor / worker not found / worker unavailable を typed close reason または stream diagnostic として返す。
- Unknown / expired cursor 時に raw session log 全体を暗黙送信して復旧しない。Backend は fresh Snapshot から再同期する。
- Transcript projection polling endpoint と event stream の責務を分ける。

### Backend proxy / permission boundary

- Browser direct Runtime access は想定せず、Backend client が Runtime WS を購読する。
- Backend は Runtime WS event を Browser / Web UI にそのまま tunnel しない。Backend-owned projection layer を通す。
- Backend projection layer は後続の権限制御を差し込める境界として設計する。
  - Worker observation を許可するか。
  - thinking / tool output / diagnostics などを表示・redact するか。
  - Web origin から利用可能な action affordance を出すか。
  - operation-capable command API への forward を許可するか。
- この Ticket では full auth / permission model は実装しないが、Runtime WS client / Backend proxy 型に policy hook を差し込める seam を作る。
- Web からの操作 block は Runtime WS ではなく Backend command API / Backend Web-facing proxy の責務とする。Runtime WS はそのための観測入力と projection boundary を提供する。

## Non-goals

- REST command server implementation。
- Browser-facing WebSocket protocol の完成。
- Backend Web UI implementation。
- Runtime-initiated Backend push connection。
- Full exactly-once delivery。
- Full auth / permission model。
- Runtime-wide multi-worker stream implementation。
- Raw provider trace streaming。
- Raw session storage migration。

## 受け入れ条件

- `worker-runtime` に optional `ws-server` feature がある。
- Feature disabled でも `worker-runtime` core が compile できる。
- Runtime process exposes worker-scoped WebSocket observation endpoint。
- WebSocket frame envelope includes opaque event id / cursor, worker id, and `protocol::Event` payload。
- Connect sends initial `protocol::Event::Snapshot` projection for the target Worker。
- Live stream forwards Worker event bus `protocol::Event` payloads rather than a parallel Worker output model or Runtime-side event subset。
- Runtime WS does not create `protocol::Method` tunnel / command / mutation channels or synthesize request/reply events。
- Backend client can reconnect with cursor / last event id semantics at the protocol level。
- Unknown cursor / expired cursor / worker not found are typed errors or stream diagnostics。
- Backend-facing Runtime WS is not Browser-facing authority and does not expose Runtime endpoint / credential / raw socket / session path to Browser。
- Backend proxy / projection boundary has an explicit seam for later observe/command permission checks and redaction policy。
- WebSocket event stream tests cover connect, initial Snapshot, live event delivery, cursor resume, duplicate-safe ids, expired/unknown cursor diagnostics, and worker-scoped filtering。
- `cargo test -p worker-runtime --features ws-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
