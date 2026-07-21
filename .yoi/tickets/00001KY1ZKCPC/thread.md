<!-- event: create author: assistant at: 2026-07-21T09:20:07Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: item_edit author: assistant at: 2026-07-21T09:42:18Z -->

## 項目更新

Ticket item updated: title, body.

---

<!-- event: item_edit author: assistant at: 2026-07-21T09:47:10Z -->

## 項目更新

Ticket item updated: title, body.

---

<!-- event: plan author: assistant at: 2026-07-21T09:55:56Z -->

## Plan

Implementation started.

Plan for this pass:
- Inspect local Worker protocol transport and current Backend/frontend HTTP operation mappings.
- Add a Backend/runtime protocol command transport that accepts existing `protocol::Method` payloads rather than per-operation HTTP endpoints.
- Route TUI Backend Runtime client through that protocol transport for run/notify/completion/cancel/pause/resume/compact/rewind operations.
- Route browser Console operations through the same protocol transport and stop using HTTP operation endpoints as the normal path.
- Keep existing HTTP endpoints only as compatibility where needed, not as frontend/TUI authority.

---

<!-- event: implementation_report author: assistant at: 2026-07-21T10:33:12Z -->

## Implementation report

Implementation update:

Implemented the first full protocol-transport pass for Backend/runtime Worker controls.

Backend/runtime:
- Added `Runtime::send_protocol_method(...)` in `worker-runtime`.
  - `Method::ListCompletions` returns direct `Event::Completions` for same-command-connection semantics.
  - Other methods are delivered unchanged to the execution backend.
- Added `WorkerExecutionBackend::dispatch_method(...)` and concrete support in `WorkerRuntimeExecutionBackend`.
  - This accepts existing `protocol::Method` values instead of translating through operation-specific HTTP request DTOs.
- Added runtime command WebSocket:
  - `/v1/workers/{worker_id}/protocol/ws`
  - accepts raw `protocol::Method` JSON frames
  - returns raw direct `protocol::Event` JSON frames where needed
- Added workspace-server command WebSocket:
  - `/api/runtimes/{runtime_id}/workers/{worker_id}/protocol/ws`
  - `/api/w/{workspace_id}/runtimes/{runtime_id}/workers/{worker_id}/protocol/ws`
- Added `RuntimeRegistry::send_protocol_method(...)` and embedded runtime forwarding.

TUI client:
- Reworked `BackendRuntimeClient` so `send(&Method)` no longer maps to HTTP `/input`, lifecycle, or unsupported diagnostics.
- It now opens a command WS to `/protocol/ws` and sends cloned raw `protocol::Method` values.
- Direct command response events are merged into the existing event queue alongside observation WS events.
- This removes the previous unsupported handling for pause/resume/compact/completions/rewind at the TUI client transport layer.

Browser Console:
- Added command WS connection to `/protocol/ws` while keeping `/events/ws` as observation stream.
- Removed normal operation use of HTTP `POST /input` and `POST /completions` from the Console page.
- Composer send now maps to protocol methods:
  - user input -> `Method::Run` with protocol `Segment[]`
  - system input -> `Method::Notify { auto_run: true }`
  - `:compact` -> `Method::Compact`
  - `:rewind` -> `Method::ListRewindTargets`
  - `:peer` -> `Method::RegisterPeer`
- Completion lookup now sends `Method::ListCompletions` over command WS and waits for direct `Event::Completions`.
- Added frontend controls for cancel / pause / resume / compact / rewind target request / rewind to target.

Compatibility note:
- Existing HTTP operation endpoints remain in backend/server as compatibility for now, but the TUI Backend client and browser Console normal path now use protocol transport.

Verification run under `nix develop`:
- `cargo fmt -- --check`
- `cargo check -p worker-runtime -p yoi-workspace-server -p client -p tui`
- `cargo test -p client backend_runtime --lib`
- `cargo test -p yoi-workspace-server registry_worker_list_can_be_scoped_by_runtime_id --lib`
- `cargo test -p worker-runtime --features ws-server --lib worker_backend`
- `git diff --check`

Frontend check note:
- `web/workspace/package.json` currently has no check script and local frontend dependencies are not installed in this worktree.
- Attempted temporary `npm exec --package svelte-check --package typescript -- svelte-check`, but the tool bootstrap failed before checking project code.
- The Svelte page was updated against the generated protocol types in `web/workspace/src/lib/generated/protocol.ts`, but a frontend-native check still needs to be run in an environment with the frontend deps installed.

---
