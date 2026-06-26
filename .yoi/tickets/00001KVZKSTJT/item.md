---
title: 'Runtime/Backend WebSocket observation proxyを実装する'
state: 'inprogress'
created_at: '2026-06-25T14:44:02Z'
updated_at: '2026-06-26T05:02:45Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T20:34:20Z'
---

## 背景

Runtime command は REST/HTTP でよいが、Worker output / status / transcript update を Backend / Web / future TUI が追うには observation transport が必要になる。Runtime から Backend へ能動接続する相互型は v0 では採用せず、Backend が Runtime の event stream に接続し、Client は Backend に接続する形にする。

この Ticket では `Runtime -> Backend -> Client` の WebSocket observation proxy を実装する。`worker-runtime` process は Worker event stream を WebSocket で公開し、Backend は登録済み Runtime handle に対する WS client として購読し、Browser / future TUI は Backend-owned client-facing WS に接続する。Backend は Runtime endpoint / token / socket path / session path を Client に渡さない。

## 目的

- Backend が Runtime 内 Worker の output / status / transcript update を購読できる。
- Backend が購読した Runtime event を Client-facing WS として proxy できる。
- Runtime WS は新しい Worker output model を作らず、既存 `crates/protocol` の `protocol::Event` を observation payload として流す。
- Client は `runtime_id + worker_id` を authority として Backend WS に接続し、Runtime endpoint / credential / raw socket path を扱わない。
- Runtime WS / Backend WS は command channel ではなく observation channel とする。
- 後続の Web 権限制御は、Backend proxy/projection layer に observe/filter/redact/command-forward policy を差し込める seam を作るに留める。

## 要件

### Overall proxy model

- v0 の主対象は WebSocket proxy path とする。
  - `Runtime -> Backend`: Runtime-owned Worker event stream。
  - `Backend -> Client`: Backend-owned Worker observation stream。
- Backend は登録済み Runtime handle / endpoint capability から Runtime WS に接続する。
- Remote Runtime から event を受け取り Client-facing WS に流す経路までをこの Ticket の実装対象に含める。
- Embedded Runtime の登録・接続実装そのものはこの Ticket の主対象にしない。ただし Backend proxy 型は embedded / remote のどちらの Runtime handle にも後続で接続できる形にする。
- Runtime process discovery / remote process lifecycle / dynamic registration はこの Ticket の scope 外とし、既に Registry から接続可能な Runtime handle が得られる前提でよい。

### Runtime WS server

- `worker-runtime` に `ws-server` feature を追加する。
- Feature disabled 時、core library は stream server dependency を強制しない。
- Runtime process が Worker observation event を WebSocket endpoint で公開できる。
- v0 required endpoint は worker-scoped stream とする。
  - `GET /v1/workers/{worker_id}/events/ws?cursor=...`
- Runtime-wide stream は v0 non-goal とする。ただし型や event id 設計は将来の runtime-wide stream を妨げない。
- Runtime WS endpoint は Backend-facing internal API として扱い、Browser-facing protocol ではない。
- Runtime WS は user input / command / mutation を受け付けない。
- Runtime WS 上に `protocol::Method` tunnel や arbitrary operation frame を作らない。
- Connect 時に対象 Worker の initial projection として `protocol::Event::Snapshot` 相当を最初に送る。
- Snapshot 後の live update は Worker event bus が publish する `protocol::Event` payload を forward する。

### Runtime WS envelope / payload

- Runtime WS frame は Runtime-local envelope を持ち、payload として `protocol::Event` を含める。
- Envelope は少なくとも以下を持つ。
  - Runtime-local opaque event id / cursor。
  - worker id。
  - `protocol::Event` payload。
  - optional diagnostic / stream control kind。
- `protocol::Event` は `crates/protocol` を authority とし、Runtime WS 専用の並行 event model を作らない。
- Runtime WS は `protocol::Event` の variant allowlist / subset を定義しない。
- Runtime WS が Web 表示可否を判断しない。Browser / Web UI 向けの filter / redact / projection は Backend proxy/projection layer の責務とする。
- Runtime WS の除外境界は variant subset ではなく channel / authority 境界とする。
  - `protocol::Method` tunnel を作らない。
  - user input / command / mutation frame を受け付けない。
  - raw provider trace / raw full session log / local filesystem paths / runtime credentials を authority payload として送らない。
- Method-specific reply を Runtime WS が passive event として合成しない。Worker event bus に publish された `protocol::Event` を forward する。

### Backend Runtime WS client

- Backend に Runtime WS client を実装する。
- Backend Runtime WS client は RuntimeRegistry から得た Runtime handle / connection capability を使い、Client から Runtime endpoint / credential を受け取らない。
- Backend Runtime WS client は Runtime-local envelope を読み、`runtime_id + worker_id + runtime_cursor + protocol::Event` として Backend 内部に渡す。
- Runtime unavailable、worker not found、unknown cursor、expired cursor、upstream disconnect、malformed frame を typed diagnostic として扱う。
- Backend client は upstream cursor を使って reconnect / resume できる。
- Upstream reconnect 時に exactly-once delivery は要求しない。Backend は event id / cursor で duplicate を扱えるようにする。

### Backend Client-facing WS proxy

- Backend は Client-facing Worker observation WS endpoint を公開する。
- v0 endpoint shape は実装時に既存 workspace-server routing と合わせて決めてよいが、Client-facing authority は `runtime_id + worker_id` とする。
  - 例: `GET /api/runtimes/{runtime_id}/workers/{worker_id}/events/ws?cursor=...`
- Browser / future TUI は Runtime WS に直接接続せず、Backend WS に接続する。
- Backend WS frame は Backend-owned envelope を持ち、payload として `protocol::Event` を含める。
- Backend-owned envelope は少なくとも以下を持つ。
  - Backend-local opaque cursor。
  - runtime id。
  - worker id。
  - `protocol::Event` payload。
  - optional diagnostic / stream control kind。
- Backend cursor は Client にとって opaque とする。実装は Runtime cursor を wrap / map してよいが、Client が Runtime-local cursor semantics に依存しないようにする。
- Backend proxy は v0 では原則 pass-through projection とし、`protocol::Event` を別 model に変換しない。
- Backend は Runtime WS event を raw tunnel せず、Backend-owned envelope / cursor / diagnostics を付与した proxy stream として出す。

### Cursor / backlog / ordering

- Runtime event id / cursor は Runtime local opaque id とする。
- Backend client-facing cursor は Backend local opaque id とする。
- Cursor は「最後に受け取った event id」を表し、resume 時はそれより後の event を送る。
- Delivery semantics は at-least-once とし、duplicate はあり得る。Backend / Client は id で de-dup できる。
- Ordering は worker-scoped stream 内で保持する。
- Bounded per-worker backlog を持ち、unknown cursor / expired cursor / worker not found / worker unavailable を typed close reason または stream diagnostic として返す。
- Unknown / expired cursor 時に raw session log 全体を暗黙送信して復旧しない。Backend / Client は fresh Snapshot から再同期する。
- Transcript projection polling endpoint と event stream の責務を分ける。

### Permission seam / future policy boundary

- この Ticket の主目的は proxy の実装であり、full auth / permission model は実装しない。
- Backend proxy/projection layer に後続で policy を差し込める seam を作る。
  - Worker observation を許可 / 拒否する。
  - thinking / tool output / diagnostics などを表示 / redact する。
  - Web origin から利用可能な action affordance を出す / 隠す。
  - operation-capable command API への forward を許可 / 拒否する。
- v0 の seam は pass-through default でよい。具体的な user/role permission、multi-user auth、redaction rule set は後続 Ticket に残す。
- Web からの操作 block は Runtime WS ではなく Backend command API / Backend Web-facing proxy の責務とする。

## Non-goals

- REST command server implementation。
- Embedded Runtime registration / direct-call implementation。
- Remote Runtime process lifecycle / discovery / dynamic registration。
- Browser-facing Web Console UI implementation。
- Full auth / permission model。
- Concrete redaction policy implementation。
- Runtime-initiated Backend push connection。
- Full exactly-once delivery。
- Runtime-wide multi-worker stream implementation。
- Raw provider trace streaming。
- Raw session storage migration。

## 受け入れ条件

- `worker-runtime` に optional `ws-server` feature がある。
- Feature disabled でも `worker-runtime` core が compile できる。
- Runtime process exposes worker-scoped WebSocket observation endpoint。
- Runtime WS frame envelope includes Runtime-local opaque event id / cursor, worker id, and `protocol::Event` payload。
- Runtime WS connect sends initial `protocol::Event::Snapshot` projection for the target Worker。
- Runtime WS live stream forwards Worker event bus `protocol::Event` payloads rather than a parallel Worker output model or Runtime-side event subset。
- Runtime WS does not create `protocol::Method` tunnel / command / mutation channels or synthesize request/reply events。
- Backend Runtime WS client can connect to a registered remote Runtime handle and receive Worker events。
- Backend Runtime WS client can reconnect with Runtime cursor / last event id semantics at the protocol level。
- Backend exposes Client-facing worker observation WS keyed by `runtime_id + worker_id`。
- Client-facing WS frame envelope includes Backend-local opaque cursor, runtime id, worker id, and `protocol::Event` payload。
- Client-facing WS can proxy initial Snapshot and live Worker events received from upstream Runtime WS。
- Backend cursor is opaque to Client and does not expose Runtime endpoint / credential / raw socket / session path。
- Unknown cursor / expired cursor / worker not found / upstream runtime unavailable are typed errors or stream diagnostics。
- Backend proxy/projection layer has an explicit seam for later observe/command permission checks and redaction policy, with pass-through default in this Ticket。
- WebSocket proxy tests cover Runtime WS connect, initial Snapshot, live event delivery, Backend upstream consumption, Client-facing proxy delivery, cursor resume, duplicate-safe ids, expired/unknown cursor diagnostics, upstream disconnect diagnostics, and worker-scoped filtering。
- `cargo test -p worker-runtime --features ws-server` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
