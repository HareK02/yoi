<!-- event: create author: "yoi ticket" at: 2026-07-13T10:02:48Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-13T10:04:08Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T10:04:08Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T10:04:08Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T10:04:08Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-13T10:04:08Z -->

## Implementation report

Implemented filesystem option cleanup for `worker-runtime-rest-server`.

- Kept `--store <fs>` as the only accepted store selector value.
- Added `--no-store` to represent the old ephemeral memory Runtime catalog mode.
- Changed `--fs-root` to mean the top-level filesystem-backed storage root.
- Added `--fs-worker-dir` for Worker storage, defaulting to `<fs-root>/worker`.
- Added `--fs-runtime-dir` for Runtime catalog storage, defaulting to `<fs-root>/runtime`.
- Added `--workdir-target` for Workdir materialization, defaulting to `<fs-root>/workdirs`.
- Removed the old `--worker-store-dir`, `--worker-metadata-dir`, and `--worker-runtime-base-dir` CLI options.
- Worker session store, Worker metadata store, Worker controller runtime base, and workdirless Worker root are now derived from `--fs-worker-dir`.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo check -q`
- `cargo check -q -p worker-runtime --features fs-store,ws-server`
- `cargo test -q -p worker-runtime --features fs-store,ws-server`
- `cargo test -q -p yoi-workspace-server`
- `cargo run -q -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --help`
- `cargo run -q -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --store memory` rejects the old memory store selector
- `cargo run -q -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --worker-store-dir /tmp/x` rejects the removed worker path option
- Started `worker-runtime-rest-server` with `--fs-root /tmp/yoi-runtime-fs-options-test` and verified `/v1/runtime` responds with `backend: fs_store` while using the derived runtime directory.
- `nix build .#yoi --no-link`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T10:04:09Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-13T12:05:03Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-13T12:05:03Z status: closed -->

## 完了

Ticket `00001KXDEVT13` (`Normalize worker runtime filesystem options`) はすでに `state: done` に到達していたため、workspace Dashboard から close しました。

この Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。


---
