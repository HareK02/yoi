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

<!-- event: decision author: hare at: 2026-06-25T15:51:07Z -->

## Decision

Decision update: Profile/config bundle synchronization is a separate implementation ticket.

The worker-runtime core ticket should only define the create-worker boundary:
- create request accepts a Profile selector.
- create request has an optional ConfigBundleRef placeholder.
- if no bundle is supplied, Runtime may resolve against its local builtin/default resources for v0 testing.

Full bundle sync is tracked by 00001KVZQHPNY and should cover digest/versioned bundle storage, Runtime-side profile resolution, Backend sync orchestration, and host-local secret/mount/plugin policy enforcement.


---

<!-- event: decision author: hare at: 2026-06-25T16:14:53Z -->

## Decision

Decision update: the worker-runtime core API must not be described as a vague list of capabilities.

The API surface is split into:

- Runtime management API: runtime summary/status/capabilities/diagnostics/store-allocation status.
- Worker catalog/lifecycle API: list/detail/create/stop/cancel workers inside one Runtime.
- Worker interaction API: send user input and start a run, with single-flight/busy semantics.
- Worker observation/projection API: status, bounded transcript, event cursor/subscription placeholder, usage/overview diagnostics.

The Runtime API should wrap the single-worker host functionality exposed by the renamed `worker` crate, but must not re-export all Worker internals. Socket protocol, attach details, session file layout, and legacy process details stay out of the Runtime public API.


---

<!-- event: intake_summary author: hare at: 2026-06-25T16:18:21Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:18:21Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T16:20:10Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T16:21:38Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue により本 Ticket の Orchestrator routing が明示許可された。
- Ticket body / thread には core slice の範囲、Non-goals、API 分割、profile/config bundle sync の境界、`worker` crate との責務境界が十分に記録されている。
- Relation blockers は解消済み。`00001KVZD10ED` は closed、`00001KVZG9BMS` は done。
- `queued` は本 Ticket 1件のみ、`inprogress` は 0 件。追加並列受理候補はない。
- Orchestration worktree は queue commit `30363e5e` 上で clean。既存の旧 worker-rename child worktree/Workers は残っているが、今回の child worktree path / branch / scope は分離できる。

Evidence checked:
- Ticket body: `crates/worker-runtime` core crate、memory-backed embedded Runtime API、Runtime/Worker identity、management/catalog/interaction/projection API、Non-goals、acceptance criteria。
- Ticket thread decisions: implementation order、`pod-store` / `pod-registry` を standalone `worker-store` / `worker-registry` にしない、FS/REST/event-stream/Backend integration/config-bundle sync は別 Ticket。
- Relations: depends_on `00001KVZD10ED` / `00001KVZG9BMS`; incoming dependent Tickets are later FS/REST/event-stream/config-bundle work and do not block this core slice.
- OrchestrationPlan: 新規 accepted plan `orch-plan-20260625-162107-1` を記録。
- Code map: root `Cargo.toml` workspace members/dependencies、既存 `crates/worker` single Worker host、`crates/workspace-server/src/hosts.rs` の現行 `WorkerRuntimeRegistry` / `WorkspaceWorkerRuntime` / local compatibility projection を確認。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` clean; implementation worktree target `/home/hare/Projects/yoi/.worktree/00001KVZBCQH4-worker-runtime-core` / branch `work/00001KVZBCQH4-worker-runtime-core` を採用。

IntentPacket:

Intent:
- `crates/worker-runtime` を追加し、Backend 等へ組み込める memory-backed embedded Runtime core API を実装する。

Binding decisions / invariants:
- `Runtime` は concrete domain entity。trait object を public authority として設計しない。
- API authority は `runtime_id + worker_id`。`pod_name` / socket path / session path を Runtime public API authority にしない。
- `worker` crate は当面 single Worker host として残し、Runtime core は socket / attach / session file details や全 public Worker internals を再公開しない。
- HTTP server / WebSocket/SSE server / REST command server / HTTP client / FS persistence / Backend RuntimeRegistry integration / Web Console は実装しない。
- `worker-store` / `worker-registry` standalone crate は作らない。store/allocation は Runtime internal abstraction として定義する。
- Profile/config bundle sync は別 Ticket。ここでは `ConfigBundleRef` placeholder と Profile selector 境界まで。
- Existing process/socket/session compatibility の削除・大規模移植はしない。

Requirements / acceptance criteria:
- `crates/worker-runtime` を workspace に追加し、library として HTTP/WS/FS dependency なしで使える。
- Runtime management、Worker catalog/lifecycle、Worker interaction、Worker observation/projection API が型として分離される。
- Memory-backed embedded Runtime が summary/status、worker list/detail/create、send input、stop/cancel、bounded transcript projection、event cursor/subscription placeholder、diagnostics を持つ。
- `CreateWorkerRequest` は `WorkerIntent`、Profile selector、optional `ConfigBundleRef`、requested capabilities、optional workspace/mount refs を表現する。
- `ConfigBundleRef` なしでも Runtime-local builtin/default resources の範囲で toolsなし Worker を作れる型/挙動にする。
- `cargo test -p worker-runtime`、`cargo check -p yoi`、`git diff --check`、必要に応じて `nix build .#yoi --no-link` が通る。

Implementation latitude:
- Module分割、型名、内部 store/allocation trait/struct の詳細、event cursor/subscription placeholder の最小実装、memory worker の transcript/event 表現は Coder が既存コード規約に合わせて選んでよい。
- v0 は actual LLM/provider integration なし、または toolsなし minimal Worker projection でよい。ただし acceptance criteria の create/send/stop/cancel/projection observable behavior はテストで示す。

Escalate if:
- `worker` crate の public API 大規模変更や socket/session compatibility 変更が必要になる。
- HTTP/WS/FS/Backend integration/config bundle sync を実装しないと acceptance を満たせないと判明する。
- `pod-store` / `pod-registry` の削除または standalone rename が必要になりそうになる。
- Runtime public API authority に socket/session/path identity を混ぜる必要が出る。

Validation:
- `cargo fmt --all`
- `cargo test -p worker-runtime`
- `cargo check -p yoi`
- `git diff --check`
- 依存/packaging変更があるため可能なら `nix build .#yoi --no-link`

Current code map:
- Add: `crates/worker-runtime/**`。
- Update: root `Cargo.toml` workspace members/default-members/dependencies, `Cargo.lock` as needed, docs/tests only if needed for public API clarity。
- Reference only as needed: `crates/worker/**` for single Worker host boundary, `crates/workspace-server/src/hosts.rs` for existing Backend registry/projection context。
- Do not implement network/server/fs integration in this Ticket.

Critical risks / reviewer focus:
- Scope creep into REST/WS/FS/Backend integration。
- Runtime API leaking legacy `pod_name` / socket/session authority。
- Re-exporting or duplicating too much of `worker` internals instead of a bounded Runtime API。
- Store/allocation becoming standalone `worker-store` / `worker-registry` crates instead of internal boundaries。
- Config bundle sync accidentally implemented instead of placeholder boundary。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T16:21:46Z from: queued to: inprogress reason: routing_accepted_unblocked_worker_runtime_core field: state -->

## State changed

Routing decision: implementation_ready。

Dashboard Queue による human-authorized routing を受け、Ticket body/thread、relations、orchestration plan、workspace state を確認した。`00001KVZD10ED` は closed、`00001KVZG9BMS` は done で relation blockers は解消済み。queued は本 Ticket 1件のみ、inprogress は 0 件。accepted plan と IntentPacket を thread / orchestration plan に記録済み。

これ以降、worktree creation / coder/reviewer Worker routing などの implementation side effect に進める。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-25T16:23:24Z -->

## Implementation report

Implementation kickoff:

- Worktree created: `/home/hare/Projects/yoi/.worktree/00001KVZBCQH4-worker-runtime-core`
- Branch: `work/00001KVZBCQH4-worker-runtime-core`
- Coder Worker spawned: `yoi-coder-00001KVZBCQH4-worker-runtime`

Scope notes:
- Coder has read access to the implementation worktree and non-recursive workspace root identity path needed by runtime startup.
- Coder write scope is intentionally narrow: `crates/**`, root `Cargo.toml`, and root `Cargo.lock` only.
- Coder is instructed not to modify Ticket records and not to operate in root/orchestration worktrees.

Startup note:
- The first spawn attempt failed before child creation because the runtime workspace root path itself was not readable in the delegated scope. Retried with non-recursive read on `/home/hare/Projects/yoi` plus implementation worktree read/write scope; second spawn succeeded.

---

<!-- event: implementation_report author: yoi-coder-00001KVZBCQH4-worker-runtime at: 2026-06-25T16:32:52Z -->

## Implementation report

Implementation report:

実装完了。マージ・Ticket 更新・cleanup は未実施。

Commit:
- `9b2cae32 feat: add memory worker runtime crate`

変更ファイル / API 概要:
- `Cargo.toml`
  - workspace member / default-member に `crates/worker-runtime` を追加。
  - workspace dependency に `worker-runtime` を追加。
- `Cargo.lock`
  - `worker-runtime` package entry を追加。
- `crates/worker-runtime/Cargo.toml`
- `crates/worker-runtime/src/*`

新規 crate `worker-runtime` の主な API 領域:
- `Runtime`: concrete な memory-backed embedded Runtime domain entity。`Runtime::new_memory()`, `Runtime::with_options(...)`。
- `identity`: `RuntimeId`, `WorkerId`, `WorkerRef`。Worker 操作は `runtime_id + worker_id` を持つ `WorkerRef` を authority として扱う。
- `management`: `RuntimeOptions`, `RuntimeSummary`, `RuntimeStatus`, `RuntimeLimits`。
- `catalog`: `CreateWorkerRequest`, `WorkerIntent`, `ProfileSelector`, `ConfigBundleRef`, `CapabilityRequest`, `WorkspaceRef`, `MountRef`, `WorkerSummary`, `WorkerDetail`, `WorkerStatus`。
- `interaction`: `WorkerInput`, `send_input(...)` による transcript 追記。
- `observation`: bounded transcript projection、`EventCursor`, `RuntimeEventBatch`, poll-only `EventSubscription` placeholder。
- `diagnostics`: `RuntimeDiagnostic`, `DiagnosticSeverity`。

実装範囲:
- HTTP / WS / SSE / REST / FS persistence / Backend integration / config bundle sync は未実装。
- provider/tool execution も未統合。
- config bundle なし・requested capabilities なしの tools-less Worker 作成をサポート。
- create/list/detail/send/stop/cancel/projection/events/diagnostics の unit tests を追加。

Validation results:
- `cargo fmt --all`: success
- `cargo test -p worker-runtime`: success (6 tests passed)
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: failure
  - 理由: `Cargo.lock` に新規 workspace crate `worker-runtime` が追加されたため、既存 `package.nix` の `cargoHash` / vendored lock consistency が古くなっている。
  - `package.nix` は今回の delegated write scope 外なので未変更。

Residual risks / review focus:
- `package.nix` の `cargoHash` 更新が別途必要。今回の delegated write scope が `crates/**`, root `Cargo.toml`, root `Cargo.lock` のみだったため対応していない。
- v0 は in-memory のみで、Runtime/Worker lifecycle semantics は最小実装。特に terminal Worker に対する再 stop/cancel の扱いを review 対象にする。
- Event subscription は意図通り poll-only placeholder。後続 streaming 実装時に cursor semantics が十分か確認する。

---

<!-- event: implementation_report author: yoi-coder-00001KVZBCQH4-runtime-packaging at: 2026-06-25T16:37:25Z -->

## Implementation report

Packaging follow-up implementation report:

対応内容:
- `package.nix` の `cargoHash` のみ更新。
- New hash: `sha256-RHo2b6dVClqu32wpgES/RQeBMXaqyqXZaooeSH5SveM=`

Commit:
- `593db95175ff8537af2401a5def21fc32dd4846d fix: update nix cargo hash`

Validation:
- `nix build .#yoi --no-link`: success
- `git diff --check`: success

Residual notes:
- 作業ツリーは clean。
- merge / close / cleanup は未実施。

---

<!-- event: review author: yoi-reviewer-00001KVZBCQH4-worker-runtime at: 2026-06-25T16:46:17Z status: request_changes -->

## Review: request changes

Review result: request_changes

主なブロッカー:

- `stop_worker` / `cancel_worker` の terminal lifecycle が不安定です。
  `Runtime::transition_worker` は「同じ status への再実行」だけを idempotent 扱いし、既に `Stopped` の Worker に `cancel_worker`、既に `Cancelled` の Worker に `stop_worker` を呼ぶと、terminal state を別の terminal state に上書きできます。
  - 該当: `crates/worker-runtime/src/runtime.rs:209-235`, `353-385`
  - 現状ロジック: `worker.status == status` の場合のみ早期 return、それ以外は `worker.status = status`
  - 影響: stop/cancel が terminal lifecycle として安定せず、summary の `stopped_worker_count` / `cancelled_worker_count` や event history が後続操作で意味を変えられる。
  - テストも `stop` と `cancel` を別 Worker で確認しているだけで、`stop -> cancel` / `cancel -> stop` の不変条件を覆っていません。
  - 期待: terminal Worker への反対側 terminal 操作は拒否するか、既存 terminal state を保持する idempotent 応答にし、該当テストを追加してください。

確認した範囲:

- `crates/worker-runtime` は workspace / default-members / workspace.dependencies に追加済み。
- crate 依存は `serde` / `thiserror` のみで、HTTP/WS/FS/provider 依存の追加は見当たりません。
- API は `management`, `catalog`, `interaction`, `observation`, `diagnostics`, `identity` に分離されています。
- `Runtime` は concrete entity として実装され、Worker 操作は `WorkerRef { runtime_id, worker_id }` を要求しています。pod/socket/session path を authority とする API は見当たりません。
- event cursor/subscription placeholder は `runtime_id` 検証、bounded `read_events`、`PollOnly` subscription として概ね意図に合っています。
- `CreateWorkerRequest` は `WorkerIntent`, `ProfileSelector`, optional `ConfigBundleRef`, requested capabilities, workspace/mount refs を保持しています。
- tools-less 作成と diagnostics のテストは存在します。
- `package.nix` の `cargoHash` 更新コミットは確認しました。
- `git diff --check f8d3b1cc..HEAD` は成功。
  cargo/nix の再実行は、read-only 指示と上記 blocker があるため実施していません。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T16:49:53Z -->

## Decision

Human follow-up:

ユーザー確認: 「OK、よろしく」。

運用判断:
- 現在 queued の dependent Tickets は planning 差し戻し不要として扱う。
- ただし、未解消 dependency があるものは queued のまま待機し、依存が解消したものだけ再 routing して `queued -> inprogress` を個別に記録してから implementation side effect に進む。
- 途中で concrete missing decision / information が出た場合は、勝手に固定せず Ticket thread に理由を残して停止または planning return を行う。
- 現在の最優先は `00001KVZBCQH4` の reviewer blocker 解消、再 review、merge、validation、done 記録。

---

<!-- event: implementation_report author: yoi-coder-00001KVZBCQH4-worker-runtime at: 2026-06-25T16:50:47Z -->

## Implementation report

Follow-up implementation report after review request_changes:

対応内容:
- `crates/worker-runtime/src/runtime.rs`
  - `Runtime::transition_worker` を修正。
  - `Stopped` / `Cancelled` の terminal 状態にある Worker に反対側 terminal 操作が呼ばれても、既存 terminal 状態を保持し、同じ `event_id` で ack を返すようにした。
  - 反対側 terminal 操作では追加 event を発生させない。
- 追加テスト:
  - `stop_then_cancel_preserves_stopped_terminal_state`
  - `cancel_then_stop_preserves_cancelled_terminal_state`
  - summary count と event history が反対 terminal 操作で変化しないことを検証。

Commit:
- `fbd358a1 fix: keep worker terminal lifecycle stable`

Validation:
- `cargo fmt --all`: success
- `cargo test -p worker-runtime`: success (8 tests passed)
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success（実行時点で dirty tree warning は出たが build 自体は成功）

Residual risks / review focus:
- terminal 状態への反対操作は reject ではなく「既存 terminal 状態を保持する idempotent 扱い」を選択。既存 API の ack semantics には自然だが、将来 UX/API で明示的 lifecycle conflict を出したい場合は typed error 方針へ切り替える余地がある。
- child worktree は commit 後 clean。

---
