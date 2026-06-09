Merged and completed the PodClient reader-task cleanup.

Summary:
- `PodClient` now stores the background reader task `JoinHandle` created during `PodClient::connect()`.
- `PodClient` implements `Drop` and aborts the reader task when the client is dropped.
- Dropping short-lived clients releases the Unix socket read half without waiting for remote socket close.
- Existing live-client send and receive behavior remains covered.
- No Pod socket protocol, event format, controller behavior, registry semantics, or panel polling policy changed.

Merged branch/worktree:
- Branch: `abort-podclient-reader-task-on-drop`
- Commit: `aec75b3 fix: abort PodClient reader task on drop`
- Merge commit on `develop`: `b0f37b5 merge: abort podclient reader task on drop`

Validation passed after merge:
- `cargo test -p client pod_client --lib`
- `cargo test -p client --lib`
- `cargo test -p tui pod_list --lib`
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/abort-podclient-reader-task-on-drop`.
- Deleted branch `abort-podclient-reader-task-on-drop`.

Residual note:
- `Drop` cannot await task cancellation completion, but reviewer accepted the normal `JoinHandle::abort()` pattern and focused tests verify observable socket cleanup behavior.