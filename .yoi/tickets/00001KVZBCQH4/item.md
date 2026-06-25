---
title: 'worker-runtime core crateと組み込みRuntime APIを作る'
state: 'inprogress'
created_at: '2026-06-25T12:17:05Z'
updated_at: '2026-06-25T16:49:53Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T16:20:10Z'
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
- Runtime API は transport API ではなく、`worker-runtime/lib.rs` が公開する Rust API として定義する。
- API surface は以下の責務に分ける。

#### Runtime management API

Runtime 自体の管理・観測を扱う。Worker 1体の操作とは分ける。

- `runtime_summary` / `runtime_status`。
- Runtime capabilities。
- Runtime diagnostics。
- Runtime-local store/allocation status。
- Runtime が保持している Worker 数や busy summary。
- v0 では Runtime config mutation は不要。config bundle sync も別 Ticket とする。

#### Worker catalog / lifecycle API

Runtime 内に存在する Worker の作成・一覧・停止を扱う。これは旧 `Pod` の process lifecycle をそのまま露出するのではなく、Runtime-scoped Worker lifecycle として定義する。

- `list_workers(query)`。
- `get_worker(worker_id)`。
- `create_worker(CreateWorkerRequest)`。
- `stop_worker(worker_id)`。
- `cancel_worker(worker_id)` or active run cancel。
- Unknown worker / duplicate worker / busy worker / unsupported capability を typed error にする。

`CreateWorkerRequest` は Web/Dashboard intent を直接受けない。Backend resolver 後、Runtime が解決可能な profile-oriented request とする。

- `display_name` / optional caller-provided worker id。
- `WorkerIntent`。
- `ProfileSelector`。
- optional `ConfigBundleRef`。
- requested capabilities。
- optional workspace / mount references。

Profile/config bundle sync は別 Ticket とし、この Ticket では `config_bundle` は optional placeholder として型に含める程度でよい。`config_bundle` が無い場合、Runtime-local builtin/default Profile resources の範囲で toolsなし Worker を作れるようにする。

#### Worker interaction API

Worker へ入力を送り、run を開始する経路を扱う。これは既存 `worker` crate が持つ single Worker の入力処理を Runtime 経由で呼べるようにする層であり、Worker 内部 API を無制限に継承しない。

- `send_input(worker_id, WorkerInput)`。
- v0 input は user message を最小単位とする。
- v0 は per-worker single-flight / busy reject でよい。
- acceptance result は accepted / rejected / busy / not found / failed を区別する。
- Runtime は `pod_name` / socket path / session path を input authority にしない。

#### Worker observation / projection API

Worker の状態と UI 用 projection を扱う。raw provider trace / raw full session log は Runtime public authority にしない。

- worker status / active run summary。
- bounded transcript projection。
- event cursor or subscription abstraction。
- usage / overview projection placeholder。
- diagnostics / last error。
- v0 は in-memory event log / transcript projection でよい。

#### Existing Worker APIとの関係

- `worker` crate は当面 single Worker host として残る。
- Runtime core は `worker` crate の全 public API を再公開しない。
- Runtime が公開するのは複数 Worker 管理に必要な catalog / lifecycle / interaction / projection API のみ。
- Worker 固有の socket protocol / attach details / session file details は Runtime API に漏らさない。

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
- Profile/config bundle sync implementation。
- Plugin package / grant / prompt resource synchronization。

## 受け入れ条件

- `crates/worker-runtime` が追加されている。
- `worker-runtime` core は HTTP / WS / FS store dependency なしで library として使える。
- `Runtime` concrete struct と Runtime/Worker domain types が公開されている。
- Runtime management API、Worker catalog/lifecycle API、Worker interaction API、Worker observation/projection API が型として分離されている。
- Memory-backed embedded Runtime が runtime summary/status、worker list/detail/create、send input、stop/cancel、bounded transcript projection、event cursor/subscription placeholder を持つ。
- Worker create request は Web/Dashboard intent ではなく、`WorkerIntent`、Profile selector、optional `ConfigBundleRef`、requested capabilities を表現できる。
- `ConfigBundleRef` が無い場合、Runtime-local builtin/default resources で toolsなし Worker を作れる。
- `worker` crate の socket / attach / session file details が Runtime public API に再公開されていない。
- Profile/config bundle sync は実装されていないが、後続 Ticket が接続できる型境界がある。
- `runtime_id + worker_id` が authority であり、`pod_name` / socket path / session path を authority にしない。
- Store / allocation abstraction が Runtime internal responsibility として定義されている。
- `worker-store` / `worker-registry` standalone crate は作られていない。
- `cargo test -p worker-runtime` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
