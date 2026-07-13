<!-- event: create author: "yoi ticket" at: 2026-07-13T13:29:51Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-13T13:30:39Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T13:30:39Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T13:30:39Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T13:30:39Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-13T13:38:25Z -->

## Implementation report

Stabilized cleanup plan candidate ordering and verified the not-found Workdir deletion endpoint.

Root cause:
- Cleanup plan digest includes worker/workdir candidate arrays.
- Candidate order was not normalized before digest calculation, so GET `/cleanup-plan` and POST `/cleanup-executions` could compute different digests for the same target set.
- The POST then failed with `workspace_cleanup_plan_stale`, preventing `workdir_record_delete` execution.

Changes:
- Sort worker cleanup candidates by `target_id` before digest calculation.
- Sort Workdir cleanup candidates by `target_id` before digest calculation.

Endpoint verification:
- Fetched `GET /api/w/0197a949-4b6b-7f2a-9d9a-1f87e3a4c5b6/runtimes/arc/cleanup-plan`.
- Posted the returned revision/digest and all `file_status: not_found` Workdir target ids to `POST /cleanup-executions`.
- Received HTTP 200 and four `workdir_record_delete` results with `status: deleted`.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo check -q`
- `cargo test -q -p yoi-workspace-server`
- targeted cleanup plan test
- `nix build .#yoi --no-link`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T13:38:25Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---
