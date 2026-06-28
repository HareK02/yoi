# Runtime Worker creation API field audit

調査日: 2026-06-28
対象 Ticket: `00001KW7726H9`

## 方針

未検証の field を API 案として固定しない。以下は現在の実装に存在する型と field を確認し、canonical Runtime Worker creation に残せるかを判定したもの。

参照した主な実装:

- `crates/worker-runtime/src/catalog.rs`
- `crates/worker-runtime/src/runtime.rs`
- `crates/worker-runtime/src/config_bundle.rs`
- `crates/worker-runtime/src/execution.rs`
- `crates/worker-runtime/src/fs_store.rs`
- `crates/workspace-server/src/hosts.rs`
- `crates/workspace-server/src/server.rs`
- `crates/worker/src/runtime_adapter.rs`
- `crates/protocol/src/lib.rs`

## 現在の Runtime `CreateWorkerRequest`

現在の型は `worker_runtime::catalog::CreateWorkerRequest`。

```rust
pub struct CreateWorkerRequest {
    pub intent: WorkerIntent,
    pub profile: ProfileSelector,
    pub config_bundle: Option<ConfigBundleRef>,
    pub requested_capabilities: Vec<CapabilityRequest>,
    pub workspace_refs: Vec<WorkspaceRef>,
    pub mount_refs: Vec<MountRef>,
}
```

### `intent: WorkerIntent`

- source: Workspace Backend / direct Runtime HTTP caller。
- current use: Worker summary/detail projection、worker snapshot persistence、diagnostic label。`Task.objective` は non-empty validation のみ。
- scope/access: なし。execution backend の実行条件ではない。
- visibility: Runtime metadata と Browser-facing projection に出る。
- persistence/projection: `PersistedWorkerRecord.request` と `WorkerSnapshot.request` に保存される。
- retry: idempotency には使われていない。
- validation: `Task.objective` の空文字のみ。
- verdict: canonical Runtime create request からは外す。Workspace Companion / Ticket role / Objective などは Backend launch context と initial input / tools 側で扱う。Runtime projection に必要な表示情報があれば、別途検証した display/projection field として定義する。

### `profile: ProfileSelector`

- source: Workspace Backend。現状は Browser-facing `WorkerSpawnRequest.profile` からも来得る。
- current use: ConfigBundle compatibility check、`worker::runtime_adapter::ProfileRuntimeWorkerFactory::runtime_profile` が実 Worker profile resolution に使う。
- scope/access: なし。ただし実 Worker の Profile 選択に直結する。
- visibility: Runtime metadata と Browser-facing projection に出る。
- persistence/projection: `CreateWorkerRequest` ごと worker snapshot に保存される。
- retry: 同じ selector なら同じ意味になり得るが、Profile 内容の時点依存は ConfigBundle identity で固定する必要がある。
- validation: `ConfigBundle` がある場合は bundle 内に profile があるか確認。`Named` は synced bundle がないと拒否。
- verdict: Runtime create request に残す候補。ただし Browser が自由入力する field ではなく、Backend が launch context から決定する。ConfigBundle identity と組み合わせて Profile 内容を固定すること。

### `config_bundle: Option<ConfigBundleRef>`

- source: 現状は caller が `ConfigBundleRef { id, digest }` を渡す。Workspace Backend の `WorkerSpawnRequest` にも露出している。
- current use: Runtime が `config_bundles` store から id で lookup し、digest 一致と profile compatibility を検証する。
- scope/access: なし。secret 値や raw path は含まない。
- visibility: Runtime metadata と Browser-facing projection に出る。
- persistence/projection: request として worker snapshot に保存される。ConfigBundle 本体は runtime snapshot の `config_bundles` に保存される。
- retry: id+digest は stable。`LatestForProfile` のような時点依存 selector は存在しないし入れない。
- validation: id / digest の形式、store presence、digest mismatch、profile containment。
- verdict: Runtime create request に残すなら、Backend が直前に `SyncConfigBundle` / availability check で得た id+digest だけを渡す形に限定する。Browser-facing launch request からは外す。`Option` のまま runtime default に落ちる経路は canonical path として再検証が必要。

### `requested_capabilities: Vec<CapabilityRequest>`

- source: 現状は Browser-facing `WorkerSpawnRequest.requested_capabilities` からも来得る。embedded spawn は空の場合に `read` を補う。
- current use: Runtime summary/detail の count/projection、request persistence。execution backend / Worker factory はこの field を実行制限として使っていない。
- scope/access: 現時点では access control ではない。validation は capability name non-empty のみ。
- visibility: Runtime detail に出る。
- persistence/projection: request として worker snapshot に保存される。
- retry: idempotency には使われていない。
- validation: name non-empty のみ。
- verdict: canonical Runtime create request からは外す。必要な declaration は ConfigBundle、実際の制限は既存 execution backend / tool host / scope 設定側で扱う。未システム化の requested capability を API field として残さない。

### `workspace_refs: Vec<WorkspaceRef>`

- source: caller supplied opaque refs。
- current use: Runtime detail projection と persistence。execution backend / Worker factory は使っていない。
- scope/access: 現時点では access control ではない。
- visibility: Runtime detail に出る。
- persistence/projection: request として worker snapshot に保存される。
- retry: 意味は caller convention 依存。
- validation: 実質なし。
- verdict: canonical Runtime create request から削除する。working directory / cwd / workspace root を Runtime metadata にしない。

### `mount_refs: Vec<MountRef>`

- source: caller supplied opaque refs。
- current use: Runtime detail projection と persistence。execution backend / Worker factory は使っていない。
- scope/access: 現時点では access control ではない。
- visibility: Runtime detail に出る。
- persistence/projection: request として worker snapshot に保存される。
- retry: 意味は caller convention 依存。
- validation: 実質なし。
- verdict: canonical Runtime create request から削除する。mount/materialization を Runtime create payload にしない。

## 現在の Browser-facing `WorkerSpawnRequest`

現在の型は `workspace-server::hosts::WorkerSpawnRequest`。

```rust
pub struct WorkerSpawnRequest {
    pub intent: WorkerSpawnIntent,
    pub requested_worker_name: Option<String>,
    pub acceptance: WorkerSpawnAcceptanceRequirement,
    pub profile: Option<ProfileSelector>,
    pub config_bundle: Option<ConfigBundleRef>,
    pub requested_capabilities: Vec<CapabilityRequest>,
}
```

### `intent: WorkerSpawnIntent`

- source: Browser / Workspace Backend caller。
- current use: embedded adapter が Runtime `WorkerIntent` と default Profile に変換する。
- verdict: Browser-facing launch context として残す。Runtime create request へはそのまま渡さない。

### `requested_worker_name: Option<String>`

- source: Browser-facing caller。
- current use: embedded runtime v0 では ignored diagnostic。WorkerId は Runtime sequence で割り当て。
- verdict: canonical flow で必要なら display/projection として再設計する。Worker identity authority にはしない。

### `acceptance: WorkerSpawnAcceptanceRequirement`

- source: Browser-facing caller / Backend launch policy。
- current use: embedded Runtime は `SocketReady` を拒否し、`RunAccepted` は diagnostic のみ。Runtime create API には存在しない。
- verdict: Browser/Backend launch policy として残す候補。Runtime create request field にはしない。acceptance evidence は Backend が create/input/observe 結果から構成する。

### `profile: Option<ProfileSelector>`

- source: Browser-facing caller からも渡せる。
- current use: Runtime create `profile` にそのまま渡る。
- verdict: Browser-facing API で自由に渡す field としては再検証が必要。canonical flow では Backend が role/target から決定し、Runtime create には Backend-decided profile を渡す。

### `config_bundle: Option<ConfigBundleRef>`

- source: Browser-facing caller からも渡せる。
- current use: Runtime create にそのまま渡る。
- verdict: Browser-facing API から削除する。Backend が ConfigBundle を決定・sync/check し、その結果の stable identity を Runtime create に渡す。

### `requested_capabilities: Vec<CapabilityRequest>`

- source: Browser-facing caller からも渡せる。
- current use: Runtime create にそのまま渡る。実 execution 制限には使われていない。
- verdict: Browser-facing API から削除するか Backend-only policy input に閉じる。Runtime create にそのまま流さない。

## ConfigBundle sync / store

- `ConfigBundle` は `metadata.id` と `metadata.digest` を持つ。
- `ConfigBundle::computed_digest()` が content digest を計算する。
- `Runtime::store_config_bundle()` は validate 後、runtime state の `config_bundles: BTreeMap<String, ConfigBundle>` に保存し、`ConfigBundleAvailability { reference: ConfigBundleRef { id, digest }, summary }` を返す。
- fs-store では ConfigBundle 本体は runtime snapshot に保存される。

Verdict:

- create caller が Runtime store を探索する必要はない。Backend が bundle を決めて sync/check し、返った id+digest を使う形なら妥当。
- Browser-facing launch に ConfigBundleRef を出す必要はない。
- `LatestForProfile` のような runtime-local latest selector は入れない。

## Initial input / Segment

- protocol `Segment` の既存 variant は `Text` / `Paste` / `FileRef` / `KnowledgeRef` / `WorkflowInvoke` / `Unknown`。
- `TicketRef` / `ObjectiveRef` variant は現在存在しない。
- Runtime `WorkerInput` は `kind` + `content: String`。Runtime transcript も `TranscriptEntry.content: String`。
- Runtime adapter は `WorkerInput.content` を `Segment::text(content)` にして `Method::Run` に渡している。

Verdict:

- initial input を `Vec<Segment>` に寄せる方針は protocol 慣例として妥当だが、現状の Runtime input/transcript は String なので、そのまま field を追加するだけでは不十分。
- Ticket / Objective を Segment として渡すなら、既存 variant で text にするか、protocol に record ref variant を追加する必要がある。未実装の `TicketRef` / `ObjectiveRef` を API field として前提にしない。
- `System` initial message は不要。role prompt / system-level instruction は Profile / ConfigBundle 側で解決する。

## Execution / workspace scope

- `WorkerExecutionSpawnRequest` は `worker_ref`、`request: CreateWorkerRequest`、`context: WorkerExecutionContext` を持つ。
- `WorkerExecutionContext` は observation publish と worker_ref を持つだけで、cwd / workspace root / scope を持たない。
- `worker::runtime_adapter::ProfileRuntimeWorkerFactory` は `workspace_root` と `cwd` を factory に保持し、request field から per-worker cwd を受け取っていない。
- Workspace Backend の embedded runtime 初期化は `WorkerRuntimeExecutionBackend::from_workspace(config.workspace_root.clone())`。

Verdict:

- 現在は per-worker execution binding API は存在しない。
- `ExecutionBindingRef` は最終型名ではなく、今の Ticket では「Runtime create payload に raw cwd/workspace/tool scope を入れない」という境界だけを確定する。
- per-worker cwd / tool scope が必要なら、まず execution backend / factory boundary を設計・実装する必要がある。

## Worker creation persistence / transaction

- worker-runtime fs-store は runtime snapshot、worker snapshot、event、transcript を保存する。
- worker snapshot は現在 `CreateWorkerRequest` 全体を保存する。
- `Runtime::create_worker()` は現在、Worker record を insert/persist してから execution backend spawn を呼ぶ。
- spawn が rejected/errored になっても Worker は残り、execution status に失敗が記録される。
- execution backend がない Runtime でも Worker は catalog-only として作れる。

Verdict:

- canonical invariant として「input-capable Worker は execution backend 接続済み」を守るには、現在の create order / failure handling を変更する必要がある。
- 永続化しない選択肢はない。検証すべきなのは、既存正規 store/projection のどこに何を保存し、spawn failure をどう診断表示するか。
- catalog-only Worker を通常 Worker として公開する現経路は canonical path では禁止または explicit non-input-capable diagnostic に分類する。

## 現時点で API field として固定してよいもの / だめなもの

固定してよい境界:

- Runtime create は WorkerId を発行する唯一の経路にする。
- Runtime create は Browser-facing launch とは別の内部境界にする。
- Runtime create は synced ConfigBundle identity を検証する。
- Runtime create は raw workspace path / cwd / socket / credential / session path を受け取らない。
- Runtime create は initial input を Worker creation transaction と同じ単位で扱う。
- Runtime create は execution backend に接続できない Worker を通常 input-capable Worker として公開しない。

まだ field として固定しないもの:

- `launch_id`: existing idempotency key がないため、source/persistence/retry contract を設計してから。
- `ConfigBundleRef` / digest field: Runtime-internal create では sync response 由来の id+digest が候補だが、Browser-facing には出さない。既存 store contract と migration を確認して確定。
- `ExecutionBindingRef`: 現在の execution backend boundary に存在しないため、field として固定しない。
- `Vec<Segment>` initial input: direction は妥当だが Runtime input/transcript が String なので、protocol/store変更を伴う。field だけ先に固定しない。
- `display`: 現在は intent/profile 由来の projection。必要なら projection設計として検証してから。
- `acceptance`: Backend launch policy。Runtime create field として固定しない。
