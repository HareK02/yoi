Implementation report for Ticket 00001KV0723PC

Commit: cfe411e50d7361228e509a18699477b13c4bc3e7 (`fix: avoid panel quit notice wait`)

Observed / identified delay cause:
- In `crates/tui/src/multi_pod.rs`, the workspace Panel run loop handled a completed `PendingReload` before polling terminal input.
- When the reload result requested Orchestrator queue-attention notification, the loop awaited `dispatch_orchestrator_queue_attention_notice(request)` inline.
- That dispatch path can wait on Unix socket connect/read/write operations and their timeouts. While it was awaited inline, queued terminal events such as `Ctrl+C` / `Ctrl+D` could not be observed, so an explicit Quit could be delayed by non-essential notice dispatch work.

Fix summary:
- Added `PendingQueueAttentionNotice`, a cancellable background task wrapper for queue-attention notice dispatch.
- Changed the Panel run loop to start queue-attention notice dispatch asynchronously after reload and only harvest the result when the task is already finished.
- Added explicit quit cleanup through `abort_panel_background_work_for_quit`, aborting both pending reload and pending queue-attention notice work before returning `MultiPodOutcome::Quit`.
- Also abort the background notice before foreground open/action paths that already abort reload work, so non-essential notice dispatch does not contend with user-directed operations.
- Preserved terminal/backend shutdown behavior by keeping the existing return path and relying on explicit abort plus Rust `Drop` cleanup for background tasks.

Files changed:
- `crates/tui/src/multi_pod.rs`

Regression coverage:
- Added `multi_quit_aborts_background_reload_and_notice_without_waiting`, which starts never-completing reload and notice tasks, exercises the quit abort helper under a timeout, and verifies both tasks are cancelled.
- Existing reload overlap behavior remains covered by `multi_poll_reload_does_not_overlap_in_flight_reload`.

Validation commands/results:
- `cargo fmt --check` — passed
- `git diff --check` — passed
- `cargo test -p tui multi_quit_aborts_background_reload_and_notice_without_waiting --lib` — passed
- `cargo test -p tui multi_poll_reload_does_not_overlap_in_flight_reload --lib` — passed
- `cargo check -p tui --all-targets` — passed

Residual risks / manual validation notes:
- No broad runtime-loop rewrite was done. Foreground user-directed operations that are already awaited by the Panel can still occupy the loop; this change targets the identified non-essential background notice dispatch delay path.
- I did not run an interactive manual `yoi panel` session from this Coder environment. Recommended manual check: start `yoi panel`, trigger a reload/queued Ticket condition that would dispatch queue-attention notice, then press `Ctrl+C`/`Ctrl+D` while the notice target is slow or unavailable; the Panel should exit promptly while terminal state is restored.
