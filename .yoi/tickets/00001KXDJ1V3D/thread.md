<!-- event: create author: "yoi ticket" at: 2026-07-13T10:58:32Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-13T10:59:02Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T10:59:02Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T10:59:02Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T10:59:02Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-13T11:16:48Z -->

## Implementation report

Removed text/substring-based Workdir not-found handling.

- Added `RuntimeError::WorkingDirectory(WorkingDirectoryDiagnostic)` so worker-runtime preserves typed Workdir diagnostic codes instead of flattening them into `invalid_request`.
- Worker Runtime REST errors now return Workdir diagnostic codes such as `working_directory_not_found` as `error.code` and map that code to HTTP 404.
- Remote Runtime HTTP error mapping now preserves typed error codes from Runtime error bodies instead of remapping all HTTP 404 responses to a generic worker-not-found code.
- Embedded Runtime diagnostics also preserve typed Workdir diagnostic codes.
- `workdir_status_from_runtime_miss` now uses exact `diagnostic.code == "working_directory_not_found"` rather than substring matching.
- Added tests for preserving Workdir REST error codes and for exact Workdir miss classification.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo check -q`
- `cargo test -q -p worker-runtime --features fs-store,ws-server`
- `cargo test -q -p yoi-workspace-server`
- `nix build .#yoi --no-link`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T11:16:48Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-13T12:05:01Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-13T12:05:01Z status: closed -->

## 完了

Ticket `00001KXDJ1V3D` (`Preserve typed Workdir not-found errors`) はすでに `state: done` に到達していたため、workspace Dashboard から close しました。

この Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。


---
