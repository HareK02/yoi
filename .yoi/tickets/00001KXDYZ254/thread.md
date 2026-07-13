<!-- event: create author: "yoi ticket" at: 2026-07-13T14:44:12Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-13T14:45:03Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T14:45:03Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T14:45:04Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T14:45:04Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-13T14:55:17Z -->

## Implementation report

Changed active Worker restore so it does not fetch backend profile source archives.

Root cause:
- Worker metadata already contains the resolved manifest snapshot used by `Worker::restore_from_worker_metadata_with_context`.
- `ProfileRuntimeWorkerFactory::restore_controller` still resolved `request.profile_source` before calling restore, so Runtime restart could fail if workspace-server/backend resource endpoint was not available yet.

Changes:
- Active restore now builds only a minimal builtin fallback manifest/loader and calls `Worker::restore_from_worker_metadata_with_context` directly.
- The metadata `resolved_manifest_snapshot` remains the restore authority.
- Backend profile source archive resolution is deferred to the pending/no-history fallback path only, where fresh Worker recreation still needs a manifest.

Validation:
- `cargo fmt`
- `cargo check -q`
- `cargo test -q -p worker-runtime --features fs-store,ws-server`
- `cargo test -q -p yoi-workspace-server`
- `cd web/workspace && deno task check && deno task test`
- `git diff --check`
- `nix build .#yoi --no-link`

Note: README.md had pre-existing unrelated local modifications and was not included.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T14:55:17Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---
