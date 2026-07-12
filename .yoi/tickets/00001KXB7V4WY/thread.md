<!-- event: create author: "yoi ticket" at: 2026-07-12T13:21:38Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-12T14:16:14Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-12T14:16:14Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-12T14:16:14Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-12T14:16:14Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-12T14:38:35Z -->

## Implementation report

Implemented Runtime restart-crossing Worker restore path.

- Added explicit `Restore` execution operation and `WorkerExecutionRestoreRequest` / backend restore API.
- Added Runtime restore pass for fs-backed Runtime startup after execution backend install.
- Persisted connected mappings remain stale on load until a real backend handle is restored.
- Restore success commits a live execution handle, connected status, Workdir status, and `WorkerExecutionRestored` event.
- Restore failure keeps the Worker stale and records `worker_execution_restore_failed` diagnostics without failing Runtime startup.
- Worker runtime backend can restore controllers through `RuntimeWorkerFactory::restore_controller`, using `Worker::restore_from_worker_metadata_with_context` and a conservative pending/no-history fallback.
- Spawn and restore share event bridge/handle registration behavior.

Validation:
- `cargo fmt`
- `cargo check -q -p worker-runtime`
- `cargo check -q -p worker-runtime --features fs-store`
- `cargo check -q -p worker-runtime --features fs-store,ws-server`
- `cargo test -q -p worker-runtime --features fs-store,ws-server`
- `cargo check -q`
- `nix build .#yoi --no-link`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-12T14:38:56Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---
