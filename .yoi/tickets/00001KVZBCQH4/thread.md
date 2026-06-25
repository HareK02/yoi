<!-- event: create author: "yoi ticket" at: 2026-06-25T12:17:05Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: comment author: hare at: 2026-06-25T12:21:06Z -->

## Comment

## 現状調査メモ: Pod / llm-worker / Panel / Workspace backend の Worker 扱い

### crates/pod

`pod` crate は現在、Worker 実行環境というより `yoi pod` process そのものの runtime を担っている。

主な責務:

- `entrypoint.rs`
  - `yoi pod` CLI entrypoint。
  - workspace / profile / manifest / project / store / pod name / session resume / hidden ticket-role marker を解決する。
  - Profile launch policy、ticket role policy、workflow selection、resource prompt loading、tool feature install の起点を持つ。
- `pod.rs`
  - `Pod` 構造体が session store、metadata、current state、scope、tool registry、workflow registry、in-flight events、LLM Worker を束ねる。
  - `Method::Run` を受けて history に user input を commit し、`llm_worker::Worker::run_with_callbacks` を起動する。
  - assistant/tool/reasoning/usage/error/turn_end を session log と event broadcast に反映する。
  - session persistence / snapshots / compaction / workflow invocation / pod metadata 更新が同居している。
- `controller.rs` / `ipc/server.rs`
  - Unix socket server。
  - connect 時に `Event::Snapshot` を送る。
  - JSON line method を読み、Pod controller に渡す。
  - broadcaster 経由で append / status / alert / snapshot events を client に流す。
- `in_flight.rs`
  - attach mid-stream 用の transient text / thinking / tool-call block accumulator。
  - session log authority ではなく socket snapshot/event 用の live projection。

Runtime crate へ移す候補:

- Worker lifecycle state / busy handling / input dispatch / event projection。
- in-flight / transcript projection の汎用概念。
- Worker event / method / status の domain model。

Pod-specific に残す候補:

- `yoi pod` CLI entrypoint。
- Unix socket protocol compatibility。
- pod metadata / runtime dir / stderr ready handshake。
- session jsonl layout compatibility。
- Profile / manifest discovery の既存 startup path。

### crates/llm-worker

`llm-worker` は Pod process とは独立した LLM turn executor に近い。

主な責務:

- `Worker` が model/provider config、history、tool registry、workflow registry、memory config、prompt config、retry/continuation policy を保持する。
- `run_with_callbacks` が 1 user turn を LLM provider に投げ、stream events を callbacks に渡す。
- tool-call loop、tool execution、reasoning / usage / continuation / retry / compaction safety など、実際の LLM turn semantics を持つ。
- `CallbackHandler` / `RunCallbacks` により、Pod 側が session persistence / event broadcast / in-flight tracking を差し込む。

Runtime crate へ移す候補:

- 「Worker に input を送り response/events を得る」上位 lifecycle。
- usage / overview projection。

そのまま再利用する候補:

- provider transport / request serialization / streaming event parsing。
- tool-call loop / history-aware retry / continuation。
- callbacks abstraction。

注意点:

- `llm-worker::Worker` は既に比較的 embeddable だが、現在は `pod::Pod` が session store・event・metadata・scope と強く結合して使っている。
- Backend internal Runtime は、Pod を経由せず `llm-worker::Worker` を直接持てる可能性がある。

### crates/client

`client` crate は既存 Pod process / socket client の adapter 部分を持つ。

主な責務:

- `spawn.rs`
  - `PodProcessLaunchConfig` / `PodProcessLaunchOptions`。
  - `yoi pod` process を起動し、stderr の `YOI-READY` と socket connectability を acceptance evidence とする。
- `pod_client.rs`
  - Unix socket に接続し、connect-time snapshot / alert を drain してから method を送る。
  - one-shot Pod client。

Runtime model 上の位置付け:

- `LocalProcess` / legacy Pod adapter が使う process-backed compatibility layer。
- `worker-runtime` lib の core semantics ではなく、process transport adapter 側。

### Panel / TUI

Panel / TUI は現在 Pod を socket / metadata ベースで扱う。

例:

- dashboard companion send path は Companion Pod の socket path に `UnixStream::connect` し、connect-time Snapshot/Alert を読んでから `Method::Run` を送る。
- `UserMessage` event を acceptance evidence とする。
- Panel の role/session claim や ticket row 操作は既存 Pod / role launch helper に寄っている。

Runtime model への移行方向:

- TUI/Panel は直接 socket path を authority とせず、Backend / Runtime API 経由で `runtime_id + worker_id` に input を送る方向へ移す。
- 既存 local Pod attach は compatibility path として残す。

### Workspace backend

Workspace backend は現在 `WorkerRuntimeRegistry` / `LocalPodRuntime` 相当を持つが、実行 Runtime ではなく local Pod metadata projection が中心。

主な現状:

- `crates/workspace-server/src/hosts.rs`
  - `WorkspaceWorkerRuntime` trait、`WorkerRuntimeRegistry`、`LocalPodRuntime` が存在する。
  - `LocalRuntimeBridge = LocalPodRuntime` alias が残る。
  - `/api/hosts`, `/api/workers`, `/api/hosts/{host_id}/workers` は registry 経由で worker summaries を返す。
  - LocalPodRuntime は `.yoi/pods/*/metadata.json` を読み、runtime/worker projection を作る。
- `spawn_worker` 等の typed shape はあるが、実 operation は unsupported / pending に近い。
- Backend internal LLM Worker runtime はまだ存在しない。

Runtime model への移行方向:

- Workspace backend の Registry は `worker-runtime::Runtime` または network Runtime client を束ねる集約境界にする。
- `LocalPodRuntime` は本来の Runtime ではなく、既存 Pod metadata/socket を Worker projection に見せる compatibility adapter として扱う。
- Backend internal Companion は `worker-runtime::Runtime` を embedded に持ち、その Runtime 内の Worker として作る。

### 初期 implementation split 案

1. `worker-runtime` crate skeleton。
   - `RuntimeId` / `WorkerId` / `WorkerRef` / status / summary / input / event / transcript projection / error 型。
   - `Runtime` concrete struct の最小 shell。
   - worker list/detail/send_input の mock or no-op capable core。
2. Backend に embedded Runtime を組み込む。
   - Workspace backend の Registry に embedded Runtime handle を登録。
   - まだ LLM は mock でもよい。
3. `llm-worker` を使った tools なし in-process Worker。
   - single-flight。
   - transcript projection。
   - usage/error projection。
4. Existing LocalPodRuntime を compatibility adapter として明示化。
   - metadata reader / socket send は adapter 側。
   - PodProcessLaunchConfig は process-backed path に閉じる。
5. `worker-runtime` binary / network API。
   - 同じ Runtime lib を起動して HTTP command + event observation API を公開する。
6. Web Companion Console MVP。
   - Backend embedded Runtime 上の companion Worker に message round-trip。


---

<!-- event: decision author: hare at: 2026-06-25T13:14:10Z -->

## Decision

## 追加調査メモ: pod-store / pod-registry の役割と移行方針

### pod-store

`pod-store` は `{data_dir}/pods/{pod_name}/metadata.json` を扱う name-keyed metadata store である。主な内容は active session/segment pointer、workspace_root、spawned/reclaimed children、peers、resolved_manifest_snapshot。

これは正規 Worker Runtime の永続化層としては粒度と authority が古い。

- identity が `pod_name` 中心。
- socket/process/session restore を前提にした metadata が混ざる。
- child/peer relation は Runtime/Worker records と orchestration records に分解すべき。
- session pointer は Runtime-local transcript/run projection として扱うべき。

短期対応として `worker-store` に rename するが、最終的には `worker-runtime` 内部の persistence module に統合する。

### pod-registry

`pod-registry` は `<runtime_dir>/pods.json` の flock-protected live allocation table である。主な内容は pod_name、pid、socket path、scope allow/deny、delegated_from、segment_id。

これは Backend の RuntimeRegistry とは別物で、旧 local Pod process 群の machine-wide scope lock / delegation / stale reclaim である。Runtime model では、Worker allocation は Runtime 内部の責務になる。

- 同一 Runtime 内 Worker の scope conflict は Runtime の allocation manager が扱う。
- remote Runtime の allocation は remote Runtime 側の authority。
- host-level conflict が必要な場合も Pod registry ではなく Runtime/host allocation model として設計する。

短期対応として `worker-registry` に rename するが、最終的には `worker-runtime` 内部の allocation / scope_registry module に統合する。

### 決定

- `pod-store` / `pod-registry` は正規 architecture では役目を終える。
- ただし即削除ではなく、まず `worker-store` / `worker-registry` へ rename し、Pod terminology を active API から外す。
- 後続 Ticket で standalone crates を削除し、`worker-runtime` 内部 module に統合する。
- 後方互換 alias / old path migration は設けない。


---

<!-- event: decision author: hare at: 2026-06-25T13:25:49Z -->

## Decision

Implementation order is now recorded with depends_on relations:

1. 00001KVZD10ED llm-worker crateをllm-engineへ改名する
2. 00001KVZEHJG1 pod-store/pod-registry crateをWorker用語へ改名する
3. 00001KVZBCQH4 組み込み/ネットワーク対応Worker Runtime crateを作る
4. 00001KVZEHJGT worker store/registryをworker-runtimeへ統合する
5. 00001KVZ9JGK0 Backend内蔵Companion RuntimeとWeb Console MVP

The key boundary is that Backend RuntimeRegistry aggregates Runtime instances, while worker store/allocation belongs inside worker-runtime after the transitional rename step.


---

<!-- event: decision author: hare at: 2026-06-25T13:30:09Z -->

## Decision

Decision update: skip the standalone worker-store / worker-registry rename step.

The implementation order is now:

1. 00001KVZD10ED llm-worker crateをllm-engineへ改名する
2. 00001KVZBCQH4 組み込み/ネットワーク対応Worker Runtime crateを作る
3. 00001KVZ9JGK0 Backend内蔵Companion RuntimeとWeb Console MVP

`pod-store` and `pod-registry` should be handled during the worker-runtime implementation itself: do not create standalone `worker-store` / `worker-registry` transitional crates. Move their relevant responsibilities directly into worker-runtime internal persistence/allocation modules, and leave Pod-specific compatibility as adapter detail only where still needed.


---

<!-- event: decision author: hare at: 2026-06-25T13:43:31Z -->

## Decision

Implementation order update:

1. 00001KVZD10ED llm-worker crateをllm-engineへ改名する
2. 00001KVZG9BMS pod crateをworker crateへ改名する
3. 00001KVZBCQH4 組み込み/ネットワーク対応Worker Runtime crateを作る
4. 00001KVZ9JGK0 Backend内蔵Companion RuntimeとWeb Console MVP

The pod -> worker step is a rename, not a rewrite. Runtime creation absorbs the former pod-store / pod-registry responsibilities directly into worker-runtime internal persistence/allocation modules; do not create standalone worker-store / worker-registry crates.


---

<!-- event: decision author: hare at: 2026-06-25T14:41:12Z -->

## Decision

Decision update: worker-runtime should separate the embeddable Runtime core from optional persistence and network transports.

- `worker-runtime/lib.rs` owns Runtime semantics and can be embedded by Backend.
- `worker-runtime/main.rs` is only a Runtime process wrapper around the same Runtime.
- Use features so embedding the library does not force FS store / HTTP server / WebSocket server dependencies.
- v0 persistence should support memory store for embedded use and fs-store for standalone Runtime process use.
- Backend <-> remote Runtime should be Backend-initiated: Browser -> Backend -> Runtime. Browser must not talk to Runtime directly.
- Commands should be REST/HTTP. Observation should be REST polling, SSE, or WebSocket; REST server and WS/SSE server implementation may be split from core crate creation.
- Runtime-initiated persistent connection back to Backend is not a v0 requirement because it complicates session, auth, reconnect, and delivery semantics.


---

<!-- event: decision author: hare at: 2026-06-25T14:48:23Z -->

## Decision

Decision update: split the former broad worker-runtime ticket into implementation-sized tickets.

Current order:

1. 00001KVZD10ED llm-worker crateをllm-engineへ改名する
2. 00001KVZG9BMS pod crateをworker crateへ改名する
3. 00001KVZBCQH4 worker-runtime core crateと組み込みRuntime APIを作る
4. 00001KVZKST83 worker-runtimeにFS永続化featureを追加する
5. 00001KVZKSTE2 worker-runtimeにREST command serverを追加する
6. 00001KVZKSTJT worker-runtimeにevent stream serverを追加する
7. 00001KVZKSV6C Backend RuntimeRegistryをworker-runtimeへ接続する
8. 00001KVZ9JGK0 Backend内蔵Companion RuntimeとWeb Console MVP

The core ticket must not absorb FS persistence, REST server, event stream server, or Backend remote client integration. Those are separate implementation tickets.


---
