<!-- event: create author: "yoi ticket" at: 2026-07-11T06:51:23Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-11T06:52:17Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T06:52:17Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T06:52:17Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T06:52:17Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-11T07:01:17Z -->

## Implementation report

Archived registry-only Workers no longer render as Console links.

- Added shared `canOpenWorkerConsole` helper that rejects `state: archived` and `implementation.kind: backend_worker_registry`.
- Sidebar renders archived Workers as disabled readonly rows instead of anchors.
- Workers page shows `Archived` instead of `Open Console` for archived Workers.
- Added unit coverage for live vs archived Console availability.

Validation:
- cd web/workspace && deno task check
- cd web/workspace && deno task test
- git diff --check
- nix build .#yoi --no-link


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T07:01:17Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---
