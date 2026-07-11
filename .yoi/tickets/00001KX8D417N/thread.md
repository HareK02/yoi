<!-- event: create author: "yoi ticket" at: 2026-07-11T10:56:08Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-11T10:56:21Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T10:56:21Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T10:56:21Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T10:56:21Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: plan author: hare at: 2026-07-11T10:56:21Z -->

## Plan

Replace meaningful concatenated Worker/Workdir ids with opaque runtime/local ids and composite backend registry keys. Preserve display labels separately, migrate legacy registry/link rows, and validate server/runtime/frontend builds.


---

<!-- event: implementation_report author: hare at: 2026-07-11T11:25:17Z -->

## Implementation report

Implemented opaque Worker/Workdir ids and removed Backend synthetic Worker PKs.

- Runtime generated Worker ids are now opaque fixed-width hex counters without the `worker-` prefix.
- Runtime and Backend generated Workdir ids are now opaque timestamp+sequence hex ids without `backend-`, `alloc-`, repository id, or Runtime/Worker-derived components.
- Workspace Backend worker registry now uses `(workspace_id, runtime_id, runtime_worker_id)` as the primary key.
- Worker/Workdir links now reference the same composite worker key instead of `{runtime_id}/{worker_id}` synthetic strings.
- Added schema v5 migration from the legacy synthetic `worker_id` registry/link tables.
- Updated cleanup, list, sync, retention, and tests to use composite keys and keep public `WorkerSummary.worker_id` Runtime-local.

Validation:
- cargo test -q -p yoi-workspace-server
- cargo test -q
- cd web/workspace && deno task check && deno task test
- git diff --check
- nix build .#yoi --no-link


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T11:25:17Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-11T11:25:17Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-11T11:25:17Z status: closed -->

## 完了

/tmp/yoi-id-implementation-report.md


---

<!-- event: implementation_report author: hare at: 2026-07-11T12:05:24Z -->

## Implementation report

Follow-up: Worker ids are now numeric, not String-backed.

- `worker_runtime::WorkerId` is now a transparent `u64` newtype instead of `String`.
- Runtime HTTP JSON uses numeric Worker ids; URL path handling parses the path segment as `u64`.
- Workspace Backend stores `runtime_worker_id` as INTEGER in `worker_registry` and `worker_workdir_links` composite keys.
- Browser-facing Workspace `WorkerSummary.worker_id` remains a string route segment derived from the numeric id, so existing frontend route composition stays simple.
- Schema v5 migration converts legacy `worker-*` / numeric string worker ids into integer `runtime_worker_id` values before rebuilding registry/link tables.


---
