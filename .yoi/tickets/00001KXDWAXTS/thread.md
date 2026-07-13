<!-- event: create author: "yoi ticket" at: 2026-07-13T13:58:15Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-13T13:58:54Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T13:58:54Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T13:58:54Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T13:58:54Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-13T14:05:35Z -->

## Implementation report

Changed Worker cleanup execution to stop the selected Worker before Runtime deletion.

Root cause:
- Runtime `delete_worker` rejects active Workers with `worker <id> is running and must be stopped before deletion`.
- The cleanup plan can present a Worker as cleanup-eligible when the browser-facing projected state is not `running` (for example execution rejected/unconnected) while Runtime status is still active.
- Cleanup execution previously called Runtime delete directly, causing `workspace_cleanup_worker_runtime_delete_rejected`.

Changes:
- `cleanup_runtime_worker_for_execution` now calls `stop_worker` first with a cleanup reason.
- If stop is rejected, the cleanup fails with `workspace_cleanup_worker_runtime_stop_rejected` and preserves Runtime diagnostics.
- Only after accepted stop does cleanup call Runtime delete and then delete the Backend registry row.
- Unknown-worker during stop/delete remains idempotent for stale Backend registry cleanup.

Manual observation:
- Direct Runtime DELETE for worker 1 returned HTTP 400: `worker 1 is running and must be stopped before deletion`.
- Direct Runtime stop followed by delete succeeded, confirming the required operation sequence.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo check -q`
- `cargo test -q -p yoi-workspace-server`
- targeted cleanup execution test
- `nix build .#yoi --no-link`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T14:05:35Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---
