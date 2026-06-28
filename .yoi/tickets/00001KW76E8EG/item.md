---
title: 'Workspace Backendのembedded Runtimeをfs-store永続化に切り替える'
state: 'queued'
created_at: '2026-06-28T13:34:29Z'
updated_at: '2026-06-28T17:12:39Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-28T17:12:16Z'
---

## 背景

Workspace Backend の embedded Runtime は現在、`EmbeddedWorkerRuntime::new_memory_with_execution_backend(...)` で memory-backed Runtime として起動している。そのため Backend を再起動すると Runtime catalog、Worker snapshot、transcript、ConfigBundle store が失われる。

`worker-runtime` 側には既に fs-store backed Runtime が存在する。

- `Runtime::with_fs_store(...)`
- `Runtime::with_fs_store_and_execution_backend(...)`
- `FsRuntimeStoreOptions { root, runtime_id, display_name, limits }`

一方で Workspace Backend 側の embedded wrapper には fs-store backed constructor が無く、`WorkspaceApi::new_with_execution_backend(...)` は memory-backed constructor を呼んでいる。

この Ticket では、Workspace Backend embedded Runtime を `worker-runtime` fs-store に接続し、Backend restart 後も Runtime record と transcript projection が残るようにする。

## 依存関係

- `00001KW7726H9 Runtime Worker起動経路を正規のExecution/ConfigBundle経路に一本化する` の設計に従う。
- この Ticket は fs-store 接続と restart persistence を扱う。canonical create request の field 設計や旧 Pod store 削除はこの Ticket の主対象にしない。

## 調査済みの現状

### Workspace Backend 側

- `crates/workspace-server/src/hosts.rs`
  - `WorkspaceApi::new_with_execution_backend(...)` が `EmbeddedWorkerRuntime::new_memory_with_execution_backend(...)` を呼んでいる。
  - embedded spawn path は `WorkerSpawnRequest` を Runtime `CreateWorkerRequest` に変換している。
- `crates/workspace-server/src/server.rs`
  - `ServerConfig` は `workspace_root`、`database_path`、host URLs などを持つ。
  - Runtime fs-store root はまだ持っていない。
- `crates/workspace-server/src/main.rs`
  - `WorkspaceIdentity::load_or_init(...)` で workspace_id は取得済み。
  - 現状の `ServerConfig::local_dev(...)` には Runtime store root が渡されていない。
- `crates/workspace-server/Cargo.toml`
  - `worker-runtime` dependency は `features = ["ws-server"]`。
  - fs-store を使うには `fs-store` feature も有効化する必要がある。

### worker-runtime 側

- `crates/worker-runtime/src/runtime.rs`
  - `Runtime::with_fs_store_and_execution_backend(options, backend)` が存在する。
  - persisted state restore 時、Worker の live execution handle は復元せず `WorkerExecutionStatus::unconnected()` / `execution_handle: None` になる。
- `crates/worker-runtime/src/fs_store.rs`
  - store root 配下に runtime_id 別の directory を作る。
  - runtime snapshot、worker snapshot、events、transcript entry を保存する。
- `crates/worker-runtime/src/config_bundle.rs`
  - `ConfigBundle` は `metadata.id` と `metadata.digest` を持つ。
  - fs-store backed Runtime の persisted runtime state に ConfigBundle store も含まれる。

## 目的

- Workspace Backend embedded Runtime を memory-backed から fs-store backed に切り替える。
- Backend restart 後に Runtime catalog / Worker snapshot / transcript / ConfigBundle store が復元される。
- live execution handle は再起動で復元できないものとして扱い、connected Worker のように見せない。
- Runtime store root は Browser-facing API に出さない。
- `.yoi/tickets` など project record と Runtime local state を混ぜない。

## 実装方針

### 1. workspace-server で worker-runtime fs-store feature を有効化する

`crates/workspace-server/Cargo.toml` の `worker-runtime` dependency に `fs-store` feature を追加する。

```toml
worker-runtime = { workspace = true, features = ["ws-server", "fs-store"] }
```

これにより `Runtime::with_fs_store_and_execution_backend(...)` と `FsRuntimeStoreOptions` を Workspace Backend から使えるようにする。

### 2. Runtime store root を ServerConfig に持たせる

`ServerConfig` に embedded Runtime 用の store root を追加する。

候補名:

```rust
pub embedded_runtime_store_root: PathBuf
```

決定事項:

- store root は Backend local state であり、Browser-facing API に出さない。
- store root は `.yoi/tickets` 配下に置かない。
- tests は temp dir を明示的に渡す。
- CLI/default path は `WorkspaceIdentity.workspace_id` を使って workspace ごとに衝突しない path にする。

推奨 default:

```text
<YOI_DATA_DIR or YOI_HOME or ~/.yoi>/workspace-server/<workspace_id>/worker-runtime
```

`workspace-server` crate が `manifest::paths::data_dir()` を使えない場合は、実装時に `manifest` dependency を追加するか、`main.rs` で default を解決して `ServerConfig` に注入する。workspace root 直下の git-tracked project record と混ぜないことを優先する。

### 3. EmbeddedWorkerRuntime に fs-store constructor を追加する

`EmbeddedWorkerRuntime` に fs-store backed constructor を追加する。

候補名:

```rust
EmbeddedWorkerRuntime::new_fs_store_with_execution_backend(...)
```

この constructor は内部で次を呼ぶ。

```rust
Runtime::with_fs_store_and_execution_backend(
    FsRuntimeStoreOptions {
        root: embedded_runtime_store_root,
        runtime_id: embedded_runtime_id,
        display_name,
        limits,
    },
    execution_backend,
)
```

runtime_id は embedded Runtime として安定した値にする。

- 既存の `EmbeddedWorkerRuntime::runtime_id()` / embedded Runtime id 方針がある場合はそれに従う。
- workspace ごとの identity は store root 側で分けるため、runtime_id に raw workspace path を入れない。
- runtime_id が変わると既存 fs-store を復元できないため、restart 間で安定させる。

### 4. WorkspaceApi の constructor を fs-store backed に切り替える

`WorkspaceApi::new_with_execution_backend(...)` は memory constructor ではなく、fs-store constructor を呼ぶ。

変更前の趣旨:

```rust
EmbeddedWorkerRuntime::new_memory_with_execution_backend(...)
```

変更後の趣旨:

```rust
EmbeddedWorkerRuntime::new_fs_store_with_execution_backend(...)
```

この変更により、Workspace Backend 経由の embedded Runtime catalog / worker snapshot / transcript / ConfigBundle が fs-store に保存される。

### 5. restart 後の execution status を明示する

`worker-runtime` の restore は現状、Worker の live execution handle を復元せず `WorkerExecutionStatus::unconnected()` にする。これは正しい前提として扱う。

この Ticket では次を確認・必要に応じて補強する。

- Backend restart 後の Worker は catalog / transcript 上は復元される。
- live execution handle は復元済みのように見せない。
- input-capable として扱えない場合は `unconnected` / stale execution mapping / typed diagnostic として表示・API 応答される。
- restart 後に同じ WorkerId を使って fake response を生成しない。

### 6. ConfigBundle store の復元を確認する

fs-store backed Runtime は persisted runtime state に ConfigBundle store を持つ。Workspace Backend 側で embedded Runtime を fs-store に切り替えた後、Backend restart で ConfigBundle が失われないことを test で確認する。

この Ticket では ConfigBundle create API の再設計は行わない。`00001KW7726H9` の canonical create 設計と矛盾しないよう、現行 store の復元だけを確認する。

## Test plan

Focused tests を `yoi-workspace-server` 側に追加する。

### embedded Runtime fs-store restore

1. temp dir を store root として `WorkspaceApi` / embedded Runtime を起動する。
2. Runtime に ConfigBundle を store する。
3. Worker を作成し、transcript に entry を追加する。
4. WorkspaceApi / Runtime を drop する。
5. 同じ store root で WorkspaceApi / Runtime を再作成する。
6. Runtime catalog に同じ Worker が復元されることを確認する。
7. transcript entry が復元されることを確認する。
8. ConfigBundle が復元されることを確認する。
9. execution status が live connected として偽装されていないことを確認する。

### memory-backed bypass absence

- Workspace Backend の production constructor が `new_memory_with_execution_backend(...)` を呼ばないことを test または grep-resistant な unit test で確認する。
- memory-backed Runtime は test helper / explicit in-memory constructor に限定する。

### store root isolation

- store root が `ServerConfig` から注入されることを確認する。
- Browser-facing API response に store root が出ないことを確認する。
- `.yoi/tickets` 配下を Runtime fs-store root にしないことを確認する。

## 受け入れ条件

- `crates/workspace-server` が `worker-runtime` の `fs-store` feature を有効化している。
- `ServerConfig` が embedded Runtime store root を持ち、Workspace Backend 起動時に明示的に渡される。
- CLI/default path が user data 配下の workspace_id 別 Runtime store root を使う。
- `EmbeddedWorkerRuntime` に fs-store backed constructor が追加されている。
- Workspace Backend の通常 constructor が `EmbeddedWorkerRuntime::new_memory_with_execution_backend(...)` を呼ばない。
- embedded Runtime が `Runtime::with_fs_store_and_execution_backend(...)` を通って起動する。
- Backend restart 後に Runtime catalog / Worker snapshot / transcript / ConfigBundle store が復元される。
- restart 後の live execution handle は復元済みとして扱われず、unconnected / stale diagnostic として扱われる。
- Browser-facing API に Runtime store root / raw path / execution handle が漏れない。
- Focused tests が embedded fs-store restore、ConfigBundle restore、transcript restore、execution unconnected restore、memory-backed bypass absence を確認する。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。

## 対象外

- canonical Runtime Worker create request の field 確定。
- per-worker cwd / tool scope の新 API。
- remote Runtime execution provisioning。
- live execution handle の restart 後自動再接続。
- 旧 Pod store / pod-registry の削除。
