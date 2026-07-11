<!-- event: create author: "yoi ticket" at: 2026-07-11T04:08:12Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-11T04:09:06Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T04:09:06Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T04:09:06Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T04:09:06Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-11T04:33:35Z -->

## Implementation report

Removed durable worker lifecycle state from the Backend worker registry.

- `WorkerRegistryRecord` no longer has `lifecycle_state`.
- Active worker_registry CREATE/INSERT/SELECT/read paths no longer include lifecycle state.
- Added schema migration v4 to rebuild legacy worker_registry tables without the old lifecycle column.
- Registry-only Worker projection now reports `archived` instead of stale `idle`/`running`/`stopped` values.
- Live Worker merge no longer lets registry data overwrite Runtime status/state.
- Cleanup planning uses live Runtime workers for running-linked detection; registry records only provide retention/link metadata.
- Removed lifecycle_state from frontend cleanup candidate types/display.

Validation:
- cargo test -q -p yoi-workspace-server
- cd web/workspace && deno task check
- cd web/workspace && deno task test
- cargo test -q
- git diff --check
- nix build .#yoi --no-link


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T04:33:35Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-11T04:34:08Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-11T04:34:08Z status: closed -->

## 完了

Removed durable `lifecycle_state` from Backend worker registry. Runtime remains the authority for live status/state; registry-only Workers project as archived, and cleanup planning uses live Runtime worker observation for running-linked checks. Existing DBs are migrated by schema v4 to remove the legacy column.


---
