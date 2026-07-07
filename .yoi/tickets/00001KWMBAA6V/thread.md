<!-- event: create author: "yoi ticket" at: 2026-07-03T15:59:48Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-03T16:52:26Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-03T16:52:26Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-03T16:52:35Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-03T16:52:35Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-03T16:53:43Z -->

## Implementation report

実装報告。

- `RuntimeRegistry` を live mutation 可能な共有 registry にし、Settings の remote Runtime add/delete から `register_or_replace` / `unregister_if_idle` を呼ぶようにした。
- add は persisted config 書き込み前に remote runtime config を解決・構築し、成功後に active registry へ反映する。成功時は `restart_required = false` と `runtime_registry_applied` diagnostic を返す。
- delete は active registry 側に worker が残っている場合 `remote_runtime_delete_blocked` で拒否し、config は削除しない。worker が無ければ config と active registry の両方から削除する。
- Settings Test 表示は connected / verified / unchecked warning を分け、接続できた範囲が分かるようにした。
- `worker-runtime-rest-server` の default bind port を `127.0.0.1:38800` に固定し、default config コメントも同じ port に合わせた。
- remote runtime blocking HTTP client/request は async context 直下で Tokio runtime を drop しない boundary 経由にした。

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo test -p yoi-workspace-server`
- `cargo test -p worker-runtime --features ws-server`
- `cd web/workspace && deno task check && deno task test`

Skipped:
- `nix build .#yoi --no-link` は重いため、ユーザー指示により未実行。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-03T16:53:43Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: implementation_report author: hare at: 2026-07-03T17:41:08Z -->

## Implementation report

Follow-up after manual Browser test.

Observed failure:
- Browser could connect/test `arcadia-test`, but worker creation failed with `workspace_worker_create_failed`: Runtime did not accept worker creation.

Root cause:
- The standalone `worker-runtime-rest-server` started from `worker-runtime` has no `WorkerExecutionBackend`; it can serve control/catalog endpoints but cannot spawn Workers.
- The connection test previously reported spawn as unknown, while runtime launch candidates projected remote worker creation as available.

Fix:
- Runtime summary now exposes `worker_creation_available` based on whether an execution backend is attached.
- Remote Runtime capability projection uses that flag for `can_spawn_worker` / `can_accept_input`.
- Settings connection test now marks `workers.spawn` incompatible with `remote_runtime_worker_creation_unavailable` when the Runtime has no execution backend.
- Added backend test covering a control-only Runtime with no execution backend.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo test -p worker-runtime --features ws-server`
- `cargo test -p yoi-workspace-server`
- `cd web/workspace && deno task check && deno task test`

Skipped:
- `nix build .#yoi --no-link` per earlier user instruction to skip heavy build.


---

<!-- event: implementation_report author: hare at: 2026-07-03T17:53:10Z -->

## Implementation report

Follow-up after `can_spawn_worker=false` review.

User feedback:
- A registered Runtime that cannot spawn workers should not exist as a usable Runtime.

Fix:
- Removed the control-only `worker-runtime` package binary surface by disabling Cargo autobin discovery and deleting `crates/worker-runtime/src/main.rs`.
- Added `worker-runtime-rest-server` under the `worker` package, where it can attach `WorkerRuntimeExecutionBackend` to the Runtime before serving REST/WebSocket endpoints.
- The server binary now starts a worker-backed Runtime, so `/v1/runtime` reports `worker_creation_available=true` for the intended standalone process.
- Retained fail-closed capability projection for old/control-only Runtime summaries so a process without execution backend is rejected as incompatible rather than exposed as a usable candidate.

New local command:
- `cargo run -p worker --features runtime-adapter --bin worker-runtime-rest-server -- --workspace .`

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo check -p yoi`
- `cargo test -p worker-runtime --features ws-server`
- `cargo test -p worker --features runtime-adapter --bin worker-runtime-rest-server`
- `cargo test -p yoi-workspace-server`
- `cd web/workspace && deno task check && deno task test`
- Smoke: started worker-backed Runtime on `127.0.0.1:38801` and verified `/v1/runtime` contains `"worker_creation_available":true`.

Skipped:
- `nix build .#yoi --no-link` per earlier user instruction to skip heavy build.


---

<!-- event: implementation_report author: hare at: 2026-07-03T18:33:15Z -->

## Implementation report

Follow-up after dependency-boundary review.

User feedback:
- Putting the standalone Runtime server binary in `worker` was also wrong; `worker` should not depend on Runtime just to host the adapter.

Correction:
- Split the Runtime/Worker bridge into a dedicated `worker-runtime-adapter` crate.
- Moved `WorkerRuntimeExecutionBackend` / `ProfileRuntimeWorkerFactory` out of `worker` and into `worker-runtime-adapter`.
- Moved the `worker-runtime-rest-server` binary into `worker-runtime-adapter`.
- Removed the `worker` -> `worker-runtime` dependency and removed the `runtime-adapter` feature from `worker`.
- `worker-runtime` remains the Runtime domain/API crate; `worker` remains Worker implementation; `worker-runtime-adapter` is the only crate depending on both.

New local command:
- `cargo run -p worker-runtime-adapter --bin worker-runtime-rest-server -- --workspace .`

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo check -p worker`
- `cargo check -p yoi`
- `cargo test -p worker-runtime --features ws-server`
- `cargo test -p worker-runtime-adapter`
- `cargo test -p yoi-workspace-server`
- `cd web/workspace && deno task check && deno task test`

Skipped:
- `nix build .#yoi --no-link` per earlier user instruction to skip heavy build.


---

<!-- event: implementation_report author: hare at: 2026-07-03T19:01:34Z -->

## Implementation report

Correction after dependency-boundary review.

User feedback:
- A separate bridge crate is unnecessary; the Runtime should bundle/manage Workers directly.

Correction:
- Removed `worker-runtime-adapter` crate entirely.
- Moved `WorkerRuntimeExecutionBackend` / `ProfileRuntimeWorkerFactory` into `worker-runtime::worker_backend`.
- Moved the `worker-runtime-rest-server` binary back into `worker-runtime`, but kept it worker-backed rather than the old control-only process.
- `worker` no longer depends on `worker-runtime`; `worker-runtime` now depends on `worker` and owns the Runtime process that bundles Workers.
- `workspace-server` now imports `worker_runtime::worker_backend::WorkerRuntimeExecutionBackend`.

Correct local command:
- `cargo run -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --workspace .`

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo check -p worker`
- `cargo check -p yoi`
- `cargo check -p worker-runtime --features ws-server,fs-store`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo test -p yoi-workspace-server`
- `cd web/workspace && deno task check && deno task test`

Skipped:
- `nix build .#yoi --no-link` per earlier user instruction to skip heavy build.


---

<!-- event: implementation_report author: hare at: 2026-07-03T19:47:23Z -->

## Implementation report

Follow-up after Runtime generation path review.

User feedback:
- Runtime generation paths should not be duplicated; a Runtime that can be served over HTTP must be constructed explicitly with the intended Worker backend rather than implicitly from HTTP server config.

Fix:
- Removed `RuntimeHttpServer::bind`.
- Removed `serve_configured_runtime_http`.
- Removed the internal `runtime_from_config` path that built backend-less Runtime instances from `RuntimeHttpServerConfig`.
- `http_server` now serves only an already-constructed `Runtime` via `serve_runtime_http(runtime, listener, token)`.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo test -p yoi-workspace-server`

Skipped:
- `nix build .#yoi --no-link` per earlier user instruction to skip heavy build.


---

<!-- event: state_changed author: hare at: 2026-07-07T13:40:23Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-07T13:40:23Z status: closed -->

## 完了

Ticket `00001KWMBAA6V` (`Live-reload Runtime connection registry changes`) はすでに `state: done` に到達していたため、workspace Dashboard から close しました。

この Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。


---
