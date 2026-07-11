<!-- event: create author: "yoi ticket" at: 2026-07-11T06:05:38Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-11T06:06:13Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T06:06:13Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T06:06:13Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T06:06:13Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-11T06:22:05Z -->

## Implementation report

Removed duplicate Workspace Worker `status` from browser-facing Worker summaries.

- `WorkerSummary` now exposes `state` only for Worker state.
- Backend Worker projections populate only `state`; registry-only Workers remain `state: archived`.
- Frontend Worker type, sidebar, Workers page, and Console header no longer reference `worker.status`.
- Runtime/Host/Workdir/protocol statuses are unchanged where they are distinct domain concepts.

Validation:
- cargo test -q -p yoi-workspace-server
- cd web/workspace && deno task check
- cd web/workspace && deno task test
- cargo test -q
- git diff --check
- nix build .#yoi --no-link


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T06:22:05Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-11T06:22:34Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-11T06:22:34Z status: closed -->

## 完了

Removed duplicate browser-facing Worker `status` field. Workspace Worker summaries now use `state` only; Frontend Worker list/sidebar/Console no longer reference `worker.status`. Runtime, Host, Workdir, and protocol statuses remain where they are distinct concepts.


---
