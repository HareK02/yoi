<!-- event: create author: "yoi ticket" at: 2026-07-10T23:34:39Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-10T23:35:04Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T23:35:04Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T23:35:04Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T23:35:04Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-10T23:50:49Z -->

## Implementation report

Removed backend-local worker observation replay.

- BackendObservationProxy no longer stores/replays event history.
- Browser worker observation WS connects upstream directly after validating cursor syntax; any cursor is no longer matched against backend replay history.
- Runtime Snapshot remains the connection bootstrap, and subsequent frames are live upstream events only.
- Updated proxy tests to assert fresh connections receive Snapshot and not previously stored live events.

Validation:
- cargo test -q -p yoi-workspace-server
- cargo test -q
- git diff --check
- nix build .#yoi --no-link


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T23:50:50Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---
