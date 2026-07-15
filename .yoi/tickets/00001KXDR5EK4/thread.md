<!-- event: create author: "yoi ticket" at: 2026-07-13T12:45:21Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-13T12:46:05Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T12:46:05Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T12:46:05Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T12:46:05Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-13T12:56:55Z -->

## Implementation report

Typed Workdir cleanup plan status and cleanliness.

- Added `CleanupWorkdirFileStatus` and `CleanupWorkdirCleanliness` typed enums with snake_case JSON serialization.
- `CleanupWorkdirCandidate.file_status` and `.cleanliness` now use those enums instead of `String`.
- Runtime-observed `WorkingDirectoryStatusKind` is converted directly into cleanup status without string formatting.
- Registry string fields are parsed only at the DB/API boundary; unknown values become `Unknown`.
- Cleanup action policy now uses enum matches/predicates rather than string matching.
- `NotFound` remains represented as `not_found` over JSON and maps to `workdir_record_delete` internally.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo check -q`
- `cargo test -q -p yoi-workspace-server`
- `cargo test -q -p worker-runtime --features fs-store,ws-server`
- `cd web/workspace && deno task check && deno task test`
- `nix build .#yoi --no-link`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T12:56:55Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-15T16:19:30Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-15T16:19:30Z status: closed -->

## 完了

Ticket `00001KXDR5EK4` (`Type Workdir cleanup plan status`) はすでに `state: done` に到達していたため、workspace Dashboard から close しました。

この Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。


---
