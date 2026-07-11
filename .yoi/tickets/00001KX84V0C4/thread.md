<!-- event: create author: "yoi ticket" at: 2026-07-11T08:31:24Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-11T08:31:54Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T08:31:54Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T08:31:54Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T08:31:54Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-11T08:44:11Z -->

## Implementation report

Fixed Worker display name preservation.

- Browser Worker create now records the sanitized UI display name in Backend worker registry instead of immediately syncing the Runtime worker id label.
- Runtime Worker create API records `requested_worker_name` as display name when provided.
- Later Runtime observation/list sync preserves existing registry display_name and only falls back to Runtime label for never-recorded live Workers.
- Added coverage that a Worker created with empty browser display_name uses the default `Coding Worker` label in Workers list/detail while keeping worker id separate.

Validation:
- cargo test -q -p yoi-workspace-server
- cd web/workspace && deno task check
- cd web/workspace && deno task test
- cargo test -q
- git diff --check
- nix build .#yoi --no-link


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T08:44:11Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---
