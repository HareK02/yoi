<!-- event: create author: "yoi ticket" at: 2026-07-13T11:43:33Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-13T11:44:18Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T11:44:18Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T11:44:18Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T11:44:18Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-13T11:51:11Z -->

## Implementation report

Fixed not-found Workdir cleanup plan classification.

Root cause:
- Cleanup plan converted `WorkingDirectoryStatusKind::NotFound` with `format!("{:?}").to_lowercase()`, producing `notfound` instead of the canonical API/status label `not_found`.
- The action classifier only recognized `missing` / `not_found`, so the candidate fell through to `workdir_dirty_discard`, which the frontend disables by design.

Changes:
- Added explicit `workdir_status_kind_label` mapping for `WorkingDirectoryStatusKind`.
- `NotFound` now maps to `not_found`, so cleanup plan candidates become `workdir_record_delete`.

Validation:
- `cargo fmt --check`
- `git diff --check`
- `cargo check -q`
- `cargo test -q -p yoi-workspace-server`
- targeted cleanup plan test
- `nix build .#yoi --no-link`


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-13T11:51:11Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-13T12:04:57Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-13T12:04:57Z status: closed -->

## 完了

Ticket `00001KXDMM975` (`Allow not-found Workdir cleanup action`) はすでに `state: done` に到達していたため、workspace Dashboard から close しました。

この Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。


---
