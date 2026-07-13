<!-- event: create author: "yoi ticket" at: 2026-07-13T07:42:49Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-13T07:43:36Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T07:43:36Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T07:43:36Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T07:43:36Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-13T08:37:44Z -->

## Implementation report

Implemented removal of Runtime-owned runtime_id from worker-runtime.

- Removed `RuntimeId` and made worker-runtime Worker authority Runtime-local (`WorkerRef { worker_id }`).
- Removed `runtime_id` from Runtime options, summaries, event cursors/subscriptions, Worker summaries/details, Runtime errors, persisted Runtime state, and Worker snapshots.
- Removed REST server `--runtime-id`; the process exposes a single Runtime identified externally by URL/backend alias only.
- Changed REST server default store selection to durable fs-store with a default user data root, keeping `--store memory` as the explicit throwaway mode.
- Changed fs-store layout so the supplied root is the Runtime store, not `root/runtimes/<runtime-id>`.
- Added legacy single-runtime layout migration from the old `runtimes/<encoded-runtime-id>/` layout into the root store when exactly one legacy Runtime directory exists.
- Changed deterministic Worker runtime names from `runtime-<runtime_id>-<worker_id>` to Runtime-local `worker-runtime-<worker_id>`.
- Kept workspace-server runtime ids as backend/endpoint aliases and adapted embedded/remote projections to attach those aliases at the workspace boundary instead of passing them into worker-runtime.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo check -q`
- `cargo test -q -p worker-runtime --features fs-store,ws-server`
- `cargo test -q -p yoi-workspace-server`
- `cargo run -q -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --help`
- `cargo run -q -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --runtime-id arc` rejects the removed option
- started `worker-runtime-rest-server` with `--fs-root /tmp/yoi-runtime-idless-test` and verified `/v1/runtime` reports `backend: fs_store` without runtime_id
- `nix build .#yoi --no-link`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T08:37:44Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-13T12:05:05Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-13T12:05:05Z status: closed -->

## 完了

Ticket `00001KXD6VFJ8` (`Remove Runtime-owned runtime_id`) はすでに `state: done` に到達していたため、workspace Dashboard から close しました。

この Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。


---
