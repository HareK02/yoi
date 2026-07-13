<!-- event: create author: "yoi ticket" at: 2026-07-13T09:21:46Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-13T09:22:30Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T09:22:30Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T09:22:30Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T09:22:30Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-13T09:31:37Z -->

## Implementation report

Removed `worker-runtime-rest-server` `--workspace` and `--cwd` options.

- Removed process config fields and CLI parsing for workspace/cwd.
- Runtime Worker factory now uses a Runtime-local `worker-root` under the Runtime root instead of a workspace root for workdirless Workers.
- Workdir bindings still provide their materialized root/cwd when a Worker has a Workdir.
- Help no longer lists `--workspace` / `--cwd`; specifying either now fails as an unknown argument.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo check -q`
- `cargo check -q -p worker-runtime --features fs-store,ws-server`
- `cargo test -q -p worker-runtime --features fs-store,ws-server`
- `cargo test -q -p yoi-workspace-server`
- `cargo run -q -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --help`
- `cargo run -q -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --workspace /tmp` rejects the removed option
- `cargo run -q -p worker-runtime --features ws-server,fs-store --bin worker-runtime-rest-server -- --cwd /tmp` rejects the removed option
- Started `worker-runtime-rest-server` with only `--bind` and `--fs-root` and verified `/v1/runtime` responds with `backend: fs_store`
- `nix build .#yoi --no-link`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T09:31:37Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-13T12:05:04Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-13T12:05:04Z status: closed -->

## 完了

Ticket `00001KXDCGNJE` (`Remove workspace and cwd from worker runtime server`) はすでに `state: done` に到達していたため、workspace Dashboard から close しました。

この Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。


---
