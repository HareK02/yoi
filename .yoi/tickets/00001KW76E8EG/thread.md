<!-- event: create author: "yoi ticket" at: 2026-06-28T13:24:27Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-06-28T17:01:47Z -->

## Plan

Workspace Backend embedded Runtime の fs-store 永続化について詳細化した。

調査済み事実:
- `worker-runtime` には `Runtime::with_fs_store_and_execution_backend(...)` と `FsRuntimeStoreOptions` が存在する。
- `workspace-server` は現在 `worker-runtime` を `ws-server` feature のみで使っており、`fs-store` feature が有効ではない。
- `WorkspaceApi::new_with_execution_backend(...)` は `EmbeddedWorkerRuntime::new_memory_with_execution_backend(...)` を呼んでいる。
- `ServerConfig` は Runtime store root を持っていない。
- `worker-runtime` の restore は live execution handle を復元せず `WorkerExecutionStatus::unconnected()` / `execution_handle: None` にする。

実装方針:
- `workspace-server` で `worker-runtime` の `fs-store` feature を有効化する。
- `ServerConfig` に embedded Runtime store root を追加する。
- CLI/default path は user data 配下の workspace_id 別 path にする。
- `EmbeddedWorkerRuntime` に fs-store backed constructor を追加し、`Runtime::with_fs_store_and_execution_backend(...)` を通す。
- Workspace Backend の通常 constructor から memory-backed Runtime を使う経路を外す。
- restart 後は catalog / transcript / ConfigBundle を復元し、live execution handle は connected として偽装しない。


---

<!-- event: intake_summary author: hare at: 2026-06-28T17:11:08Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-28T17:11:08Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---
