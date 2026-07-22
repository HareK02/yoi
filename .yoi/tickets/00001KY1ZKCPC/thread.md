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

<!-- event: implementation_report author: assistant at: 2026-07-21T11:29:08Z -->

## Implementation report

Follow-up implementation after design clarification:

Changed the transport split so `/events/ws` no longer exists. `/protocol/ws` is now the single Worker protocol WebSocket for both directions:

- client -> runtime/backend: raw `protocol::Method` JSON frames
- runtime/backend -> client: raw `protocol::Event` JSON frames

Runtime changes:
- Removed `/v1/workers/{worker_id}/events/ws` route.
- Removed runtime-specific event WS envelope/diagnostic frame types.
- Extended `/v1/workers/{worker_id}/protocol/ws` to stream snapshot/backlog/live observation events as raw `protocol::Event` while also accepting method frames on the same socket.

Workspace Backend changes:
- Removed public `/api/.../events/ws` and `/api/w/.../events/ws` routes.
- Removed Backend client event WS envelope types.
- Workspace `/protocol/ws` now proxies/streams raw protocol events and forwards raw protocol methods on the same socket.
- Runtime observation source endpoint generation now targets runtime `/protocol/ws` instead of `/events/ws`.

Client/frontend changes:
- TUI `BackendRuntimeClient` no longer opens a separate observation socket. It uses only `/protocol/ws` for send and receive.
- Browser Console no longer opens both observation and command sockets. It uses one protocol socket for replay/live events, completions, rewind targets, and controls.
- Removed old frontend `ClientWorkerEventWs*` types.
- Updated frontend UI tests to assert protocol-only Console behavior and absence of operation HTTP paths.

Verification:
- `nix develop -c cargo fmt -- --check`
- `nix develop -c cargo check -p worker-runtime -p yoi-workspace-server -p client -p tui`
- `nix develop -c cargo test -p client backend_runtime --lib`
- `nix develop -c cargo test -p worker-runtime --features ws-server --lib protocol_ws`
- `nix develop -c cargo test -p yoi-workspace-server protocol_ws --lib`
- `nix develop -c cargo test -p yoi-workspace-server proxy_maps_runtime_worker_not_found_http_404_to_protocol_error_event --lib`
- `deno test --allow-read web/workspace/src/lib/workspace/console/worker-console.ui.test.ts` from `web/workspace`
- `git diff --check`

---

<!-- event: implementation_report author: assistant at: 2026-07-21T11:42:36Z -->

## Implementation report

Bug fix after manual TUI send failure:

Observed symptom:
- TUI send over Backend protocol reported a `does not support Worker protocol command transport` style error.

Root cause:
- `/events/ws` had been removed and `/protocol/ws` unified, but workspace-server still handled inbound client `protocol::Method` by calling `RuntimeRegistry::send_protocol_method(...)`.
- For remote runtime-backed workers, Backend must be only a protocol WS proxy. Calling the registry default method hits the unsupported path instead of tunneling the frame to the runtime's `/protocol/ws`.

Fix:
- Workspace-server now branches by observation/protocol source:
  - `RemoteWs`: proxy raw WebSocket frames bidirectionally between client `/api/.../protocol/ws` and runtime `/v1/.../protocol/ws`. Backend does not deserialize or interpret method frames in this path.
  - `Embedded`: use in-process runtime adapter for method dispatch while streaming embedded observation events.
- Added/updated test so sending `Method::ListCompletions` through the workspace-server protocol proxy returns `Event::Completions` from the runtime, proving method frames reach the runtime instead of the Backend unsupported fallback.

Verification:
- `nix develop -c cargo fmt -- --check`
- `nix develop -c cargo check -p yoi-workspace-server -p client -p worker-runtime`
- `nix develop -c cargo test -p yoi-workspace-server proxies_worker_protocol_ws_as_raw_events --lib`
- `nix develop -c cargo test -p client backend_runtime --lib`
- `nix develop -c cargo test -p worker-runtime --features ws-server --lib protocol_ws`
- `nix develop -c cargo test -p yoi-workspace-server protocol_ws --lib`
- `deno test --allow-read web/workspace/src/lib/workspace/console/worker-console.ui.test.ts`
- `git diff --check`

---

<!-- event: implementation_report author: assistant at: 2026-07-22T02:11:40Z -->

## Implementation report

Bug fix for missing local echo in TUI/Web:

Observed symptom:
- Sending from TUI/Web over unified `/protocol/ws` reached the runtime and was accepted as `protocol_method`, but the user's sent content did not appear in the Console.

Root cause:
- The old HTTP input path emitted observation events for accepted input via `input_protocol_event(...)` (`Event::UserMessage` for user input / `Event::SystemItem` for system input).
- The new raw `Method::Run` / `Method::Notify` protocol path dispatched the method to the execution backend but did not record the corresponding observation echo event.
- Therefore the unified protocol stream had assistant/runtime events, but no echo event representing the user-sent content.

Fix:
- Added protocol-method observation echo in `worker-runtime` after accepted `Runtime::send_protocol_method(...)` dispatch.
- `Method::Run` now records `Event::UserMessage { segments }`.
- `Method::Notify` now records a system input `Event::SystemItem`.
- Kept compact/list_rewind_targets/register_peer command echoes aligned with the old command-input observation shape.
- Added a runtime protocol WS regression test: send `Method::Run` over `/protocol/ws` and assert the same socket receives `Event::UserMessage` with the sent text.

Verification:
- `nix develop -c cargo fmt -- --check`
- `nix develop -c cargo check -p worker-runtime -p yoi-workspace-server -p client -p tui`
- `nix develop -c cargo test -p worker-runtime --features ws-server --lib protocol_ws_echoes_accepted_run_as_user_message`
- `nix develop -c cargo test -p worker-runtime --features ws-server --lib protocol_ws`
- `nix develop -c cargo test -p yoi-workspace-server protocol_ws --lib`
- `nix develop -c cargo test -p client backend_runtime --lib`
- `git diff --check`

---

<!-- event: implementation_report author: assistant at: 2026-07-22T02:54:28Z -->

## Implementation report

Correction after review:

The previous fix synthesized `Event::UserMessage` in `Runtime::send_protocol_method(...)`. That was the wrong layer. `UserMessage` is part of the normal Worker protocol/log output path and should not be constructed by the runtime transport layer.

Root cause refined:
- The embedded Worker backend bridge only subscribed to `WorkerHandle::event_tx`.
- Local socket clients also subscribe to `SegmentLogSink` live entries; `LogEntry::UserInput` is mapped to `Event::UserMessage` there.
- Runtime observation/protocol streaming missed that `SegmentLogSink` live-entry path, so user input echo events were absent when using runtime protocol transport.

Fix:
- Removed the runtime-side synthetic protocol-method echo from `Runtime::send_protocol_method(...)`.
- Updated `WorkerRuntimeExecutionBackend::connect_handle(...)` to bridge both:
  - direct Worker protocol events from `WorkerHandle::subscribe()`
  - live session log entries from `handle.sink.subscribe_with_snapshot()`
- Added runtime bridge mapping matching local IPC behavior:
  - `LogEntry::SegmentStart` -> `Event::SegmentRotated`
  - `LogEntry::UserInput` -> `Event::UserMessage`
  - `LogEntry::SystemItem` -> `Event::SystemItem`
  - `LogEntry::Invoke` -> `Event::InvokeStart`
- Removed the fake protocol-WS echo test that depended on synthetic runtime behavior.
- Added a unit test that locks the normal bridge mapping: live `LogEntry::UserInput` maps to `Event::UserMessage`.

Verification:
- `nix develop -c cargo fmt -- --check`
- `nix develop -c cargo check -p worker-runtime -p yoi-workspace-server -p client -p tui`
- `nix develop -c cargo test -p worker-runtime --features ws-server --lib runtime_bridge_maps_live_user_input_log_entry_to_user_message`
- `nix develop -c cargo test -p worker-runtime --features ws-server --lib protocol_ws`
- `nix develop -c cargo test -p yoi-workspace-server protocol_ws --lib`
- `nix develop -c cargo test -p client backend_runtime --lib`
- `git diff --check`

---

<!-- event: decision author: assistant at: 2026-07-22T03:41:01Z -->

## Decision

Design correction / implementation target:

Runtime protocol WS should be thin. `/v1/workers/{worker_id}/protocol/ws` is already worker-id scoped, so after worker lookup it should not reimplement Worker protocol semantics. It should connect the WebSocket framing to the same Worker protocol session behavior used by local IPC.

Current issue:
- Local IPC protocol session in `worker/src/ipc/server.rs` already knows how to combine:
  - `WorkerHandle::event_tx` protocol events
  - `SegmentLogSink` live log entries (`LogEntry::UserInput` -> `Event::UserMessage`, etc.)
  - `Method` frame reading and event writing
- Runtime WS has been recreating similar logic in `worker-runtime/src/http_server.rs` / `worker_backend.rs`.
- This duplicated logic caused drift: runtime protocol streaming initially missed `SegmentLogSink` live entries, and a bad synthetic echo workaround was added before being corrected.

Decision:
- Do not make runtime/backend interpret normal Worker protocol operations.
- Backend remains a raw protocol WS proxy for remote runtime workers.
- Runtime remains a worker-id lookup + lifecycle boundary; for protocol WS it should delegate to a shared Worker protocol session implementation.
- Local IPC and runtime WS should use the same session logic so user echo, system items, invoke events, completions, rewind, pause/resume, etc. cannot diverge.

Implementation target:
- Extract the local IPC protocol session logic into a reusable Worker protocol session helper that is not Unix-socket-specific.
- Local IPC server uses the helper.
- Runtime protocol WS uses the same helper or a thin adapter around it.
- Remove duplicated live-entry/event bridging from runtime-specific code where possible.
- Keep lifecycle/runtime management outside the protocol session helper.

---

<!-- event: implementation_report author: assistant at: 2026-07-22T03:51:04Z -->

## Implementation report

Implementation update for thin runtime protocol session:

- Added `worker::ipc::protocol_session` as the shared Worker protocol session helper.
  - `subscribe_worker_protocol_session(&WorkerHandle)` centralizes snapshot + alert + live log entry + event subscriptions.
  - `live_log_entry_event(LogEntry)` centralizes the local IPC mapping:
    - `SegmentStart` -> `Event::SegmentRotated`
    - `UserInput` -> `Event::UserMessage`
    - `SystemItem` -> `Event::SystemItem`
    - `Invoke` -> `Event::InvokeStart`
  - `dispatch_worker_protocol_method(...)` centralizes same-connection direct response handling for `ListCompletions`, forwarding other methods to `WorkerHandle::send`.
- Updated local IPC server to use the shared helper instead of owning its own mapping/subscription logic.
- Updated `worker-runtime` embedded backend bridge to use the same shared helper for live Worker/log event bridging.
- Removed the duplicated `live_log_entry_event` mapping from runtime-specific code.

Scope note:
- This keeps Backend as raw WS proxy for remote runtime workers.
- Runtime still performs worker-id lookup and execution/lifecycle boundary work, but protocol stream composition now uses the same Worker session helper as local IPC instead of maintaining a separate mapping implementation.
- Full WS socket pump and Unix JSONL pump still have transport-specific loops, but the protocol subscription/mapping/direct-response semantics are no longer duplicated.

Verification:
- `nix develop -c cargo fmt -- --check`
- `nix develop -c cargo check -p worker-runtime -p worker -p yoi-workspace-server -p client -p tui`
- `nix develop -c cargo test -p worker --lib ipc::protocol_session`
- `nix develop -c cargo test -p worker --lib ipc::server`
- `nix develop -c cargo test -p worker-runtime --features ws-server --lib protocol_ws`
- `nix develop -c cargo test -p yoi-workspace-server protocol_ws --lib`
- `nix develop -c cargo test -p client backend_runtime --lib`
- `git diff --check`

---
