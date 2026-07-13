<!-- event: create author: "yoi ticket" at: 2026-07-13T17:48:09Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-13T17:48:47Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T17:48:47Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T17:48:47Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T17:48:47Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: state_changed author: hare at: 2026-07-13T17:59:59Z from: inprogress to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-13T17:59:59Z status: closed -->

## 完了

Runtime observation snapshots now prefer the live execution backend snapshot. The real Worker backend returns a WorkerHandle-built Snapshot containing session-log entries, greeting, status, and in-flight state; the Runtime falls back to the old stub only when no live backend handle exists.

Validation:
- cargo fmt --check
- cargo check -q
- cargo test -q -p worker-runtime --features fs-store,ws-server
- cargo test -q -p yoi-workspace-server
- cd web/workspace && deno task check && deno task test
- git diff --check
- nix build .#yoi --no-link


---
