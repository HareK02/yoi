---
title: 'worker-runtime core crateと組み込みRuntime APIを作る'
state: 'planning'
created_at: '2026-06-25T12:17:05Z'
updated_at: '2026-06-25T14:48:23Z'
assignee: null
---

## 背景

Yoi は旧 `Pod` 相当の実行単位を今後 `Worker` として扱い、`Runtime` が複数 Worker を保持・操作する構造へ移行する。`llm-worker` は `llm-engine` へ改名し、既存 `pod` crate は `worker` crate へ改名する。次に必要なのは、Backend が持つ `RuntimeRegistry` ではなく、**Worker を実際に動かす環境そのものとしての Runtime** を library として定義することである。

この Ticket は `worker-runtime` 全体を一括実装する umbrella ではない。最初の実装 slice として、HTTP server、WebSocket/SSE server、FS 永続化、remote client は含めず、Backend などに組み込める `worker-runtime` core crate と memory-backed embedded Runtime API を作る。

## 要件

### Crate / feature boundary

- `crates/worker-runtime` を追加する。
- `worker-runtime/lib.rs` は embeddable Runtime core API を公開する。
- この Ticket では `worker-runtime/main.rs` / standalone Runtime process は実装しない。
- Core crate は HTTP server / WebSocket server / filesystem store dependency を強制しない。
- Cargo feature の土台だけは切ってよい。
  - `memory-store` or core default。
  - 将来の `fs-store`。
  - 将来の `http-server`。
  - 将来の `event-stream` / `ws-server`。
  - 将来の `http-client`。

### Runtime / Worker model

- Runtime は Worker を動かす環境であり、trait object ではなく concrete runtime domain entity として扱う。
- Runtime は Runtime-scoped Worker identity を使う。
  - `runtime_id`
  - `worker_id`
  - `display_name`
  - `display_ref`
- Browser / Backend / API が `pod_name` / socket path / session path を authority にしない model を定義する。
- Runtime / Worker の summary / detail / state / capability / diagnostics 型を定義する。
- UI 表示用 `worker-name@runtime-name` と API authority `runtime_id + worker_id` を分ける。

### Embedded Runtime API

- Backend などの Rust process に `Runtime` を直接組み込める。
- v0 は memory store でよい。
- API は少なくとも以下を表現する。
  - runtime summary / status。
  - worker list / detail。
  - create worker。
  - send input。
  - stop / cancel worker。
  - bounded transcript projection。
  - event subscription または event cursor。
  - usage / overview projection placeholder。
- v0 は tools なし Worker / mock or minimal engine を許容する。
- v0 は single-flight / busy reject でよい。
- raw provider trace / raw full session log を Runtime public authority にしない。

### Store / allocation core

- Memory-backed store を core に含める。
- Store API は将来 `fs-store` feature や Backend-provided store に差し替えられる境界を持つ。
- `pod-store` 相当の責務は standalone `worker-store` にせず、Runtime internal persistence abstraction として設計する。
- `pod-registry` 相当の責務は standalone `worker-registry` にせず、Runtime internal allocation / scope abstraction として設計する。
- この Ticket では full FS persistence / host-level stale reclaim は実装しない。

### Existing Worker / LLM engine boundary

- `llm-engine` は LLM turn engine として扱い、Runtime / Worker identity は持たせない。
- `worker` crate は当面 single Worker host として残り、Runtime core から直接大規模移植しない。
- Existing process/socket/session compatibility は後続 adapter / integration で扱う。

## Non-goals

- `fs-store` implementation。
- REST command server。
- SSE / WebSocket observation server。
- HTTP client / Backend RuntimeRegistry remote integration。
- Backend internal Companion Web Console。
- Existing `pod-store` / `pod-registry` crate の即時削除。
- Existing Worker process/socket/session model の削除。
- Full remote Runtime protocol。

## 受け入れ条件

- `crates/worker-runtime` が追加されている。
- `worker-runtime` core は HTTP / WS / FS store dependency なしで library として使える。
- `Runtime` concrete struct と Runtime/Worker domain types が公開されている。
- Memory-backed embedded Runtime が worker list/detail/create/send input/stop/transcript projection の最小 API を持つ。
- `runtime_id + worker_id` が authority であり、`pod_name` / socket path / session path を authority にしない。
- Store / allocation abstraction が Runtime internal responsibility として定義されている。
- `worker-store` / `worker-registry` standalone crate は作られていない。
- `cargo test -p worker-runtime` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
