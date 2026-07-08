---
title: 'Add Runtime-to-Backend resource fetch REST API'
state: 'queued'
created_at: '2026-07-08T09:12:33Z'
updated_at: '2026-07-08T10:04:10Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-08T10:04:10Z'
---

## 背景

Runtime は Worker execution host であり、Workspace filesystem を直接読まない。一方で Worker 実行中または Worker 作成時には、Backend が正本を持つ Workspace-derived resource を Runtime が取得する経路が必要になる。

例:

- Decodal Profile launch の `ProfileSourceArchive` prefetch。
- Memory / Knowledge の read/query/write tool backend。
- Ticket / Objective の read/query/update tool backend。
- Workspace metadata / repository metadata の bounded lookup。
- 将来の Backend-owned resource / prompt / policy artifact lookup。

これらを Runtime-local filesystem discovery や ad hoc endpoint で増やすと、Workspace authority、credential、redaction、audit、capability の境界が崩れる。本 Ticket は Runtime -> Backend の情報取得を、Runtime-internal REST API と Backend-issued resource handle として実装する。

## 実装順序

1. 本 Ticket `00001KX0G06VA`: Runtime -> Backend の resource fetch REST API を実装する。
2. Ticket `00001KWZ5KERY`: Decodal `ProfileSourceArchive` prefetch/verify は、本 Ticket の REST API を使って Backend から archive を取得する。
3. Ticket `00001KX0DSMPT`: Workspace/Profile settings UI/API は Decodal source model に合わせて実装する。
4. Memory / Ticket / Objective tool backend の Runtime -> Backend 接続は後続 Ticket で、この REST API 上の resource/capability として実装する。

## 実装目的

- Runtime が Workspace filesystem を読まずに、Backend-owned resource を取得できる。
- Backend が Workspace information の authority として access control / redaction / audit を持つ。
- Runtime は Backend-issued resource handle なしに任意 Workspace information を取得できない。
- Browser-facing API に Runtime -> Backend resource handle、Backend internal endpoint、credential、raw path を露出しない。
- `ProfileSourceArchive` prefetch のような Worker creation-time fetch と、Memory/Ticket/Objective のような Worker runtime-time lookup の両方に拡張できる。

## 実装内容

### 1. Resource handle model を追加する

Backend が Runtime に渡せる typed resource handle を定義する。

handle に含めるもの:

- resource kind。
- workspace id / scope id。
- resource id または content digest。
- operation: read / query / write / append / fetch archive などの最小権限。
- expiry / nonce / revision / generation。
- redaction policy / max bytes / content type。
- audit correlation id。

handle に含めないもの:

- raw Backend URL。
- Runtime endpoint / token。
- host absolute path。
- secret value。
- Browser request 由来の raw filesystem scope。

### 2. Runtime-internal Backend REST API を実装する

Runtime が Backend-owned resource を取得する REST API を追加する。v0 は request/response fetch でよく、WebSocket や persistent bidirectional channel は使わない。

想定 endpoint:

```text
POST /internal/runtime/resources/fetch
```

実装すること:

- embedded Runtime direct call path。
- remote Runtime HTTP path。
- request/response typed schema。
- runtime credential / connection authority validation。
- bounded bytes / optional streaming or chunking policy。
- timeout / cancellation / retry policy。
- missing / expired / unauthorized / unsupported resource diagnostics。
- response digest validation。

### 3. Backend resource broker を実装する

Backend は resource handle を検証し、対応する Workspace-derived resource を返す。

実装すること:

- handle validation。
- workspace id / runtime id / worker id binding validation。
- resource kind dispatch。
- redaction / max bytes / content type validation。
- audit log / diagnostic。
- stale revision / digest mismatch handling。

v0 resource kind:

- `profile_source_archive`: `ProfileSourceArchive` bytes fetch。

後続 resource kind:

- `memory_query` / `memory_read` / `memory_write`。
- `ticket_read` / `ticket_update`。
- `objective_read` / `objective_update`。
- `workspace_metadata_read`。

### 4. Runtime-side REST client / cache を追加する

Runtime は Backend resource handle を受け取り、必要な resource を REST fetch / verify / cache できる。

実装すること:

- resource fetch REST client。
- digest / revision / content type validation。
- bounded cache for immutable/digest-addressed artifacts。
- mutable resource は cache しないか、short TTL / revision validation を使う。
- diagnostics を Worker creation / tool call result に接続する。

### 5. Authority boundary tests を追加する

Runtime -> Backend fetch が Workspace filesystem bypass や raw endpoint leak にならないことを test する。

検証すること:

- Runtime が raw path を知らずに `profile_source_archive` を取得できる。
- expired/unauthorized handle は拒否される。
- wrong runtime / wrong worker / wrong workspace binding は拒否される。
- response digest mismatch は拒否される。
- Browser-facing response に resource handle / internal endpoint / credential / raw path が出ない。

## 受け入れ条件

- Runtime -> Backend resource fetch REST API が typed request/response schema として実装されている。
- Backend が resource handle を発行・検証し、v0 resource kind `profile_source_archive` を返せる。
- Runtime が resource handle から `ProfileSourceArchive` を REST fetch / verify / cache できる。
- embedded Runtime direct call path と remote Runtime HTTP path が同じ resource contract を使う。
- Runtime は Workspace filesystem を読まず、Backend resource handle なしに Workspace-derived resource を取得できない。
- expired handle、unauthorized handle、workspace/runtime/worker mismatch、missing resource、digest mismatch、oversized response は typed diagnostic になる。
- Browser-facing API に Backend internal endpoint、resource credential、raw path、resource handle が露出しない。
- Focused tests が direct path、remote path、authorization mismatch、digest mismatch、Browser redaction を確認する。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `yoi ticket doctor` が通る。
- `nix build .#yoi --no-link` が通る。

## 非目標

- Memory / Ticket / Objective tools の実装本体。
- ProfileSourceArchive tar format / Decodal resolver 本体。これは `00001KWZ5KERY` で扱う。
- Workspace/Profile editing UI。これは `00001KX0DSMPT` で扱う。
- WebSocket / persistent bidirectional resource channel。
- Secret value synchronization。
- Plugin package manager / artifact sync。
