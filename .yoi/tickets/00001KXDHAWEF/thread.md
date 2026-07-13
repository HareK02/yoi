<!-- event: create author: "yoi ticket" at: 2026-07-13T10:45:59Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-13T10:46:30Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T10:46:30Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T10:46:30Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T10:46:30Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-13T10:51:20Z -->

## Implementation report

Changed workspace-server backend operation error reporting to preserve backend messages instead of replacing them with the fixed `backend-private details were omitted` text.

- `sanitize_backend_error` now returns the original message.
- Updated the existing test to assert backend operation details are preserved.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo test -q -p yoi-workspace-server`
- `nix build .#yoi --no-link`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T10:51:20Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---
