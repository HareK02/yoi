<!-- event: create author: "yoi ticket" at: 2026-07-18T12:38:47Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-18T12:39:18Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T12:39:18Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T12:39:18Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T12:39:18Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-18T12:49:12Z -->

## Implementation report

Worker lifecycle と Workdir lifecycle を分離した。

変更点:
- `stop_worker` から `materializer.cleanup(binding)` を削除し、Worker stop/delete で Workdir 実体を削除しないようにした。
- live execution map の `working_directory` field は stop cleanup 用だけだったため削除した。
- `spawn_worker` の failure rollback 対象を「この spawn request で新規 materialize した binding」だけに限定した。
- rollback は `cleanup(binding)` ではなく `cleanup_working_directory(id)` を使い、worktree 実体だけでなく record root も消して corrupted 残骸を残さないようにした。
- 既存 Workdir bind の spawn failure では Workdir を cleanup しない。

追加テスト:
- `stopping_and_deleting_worker_preserves_bound_working_directory`
- `spawn_failure_with_existing_working_directory_preserves_workdir`
- `spawn_failure_with_new_materialization_rolls_back_workdir_record`

検証:
- `cargo test -p worker-runtime --features ws-server,fs-store stopping_and_deleting_worker_preserves_bound_working_directory --quiet`
- `cargo test -p worker-runtime --features ws-server,fs-store spawn_failure_with_existing_working_directory_preserves_workdir --quiet`
- `cargo test -p worker-runtime --features ws-server,fs-store spawn_failure_with_new_materialization_rolls_back_workdir_record --quiet`
- `cargo test -p worker-runtime --features ws-server,fs-store worker_backend --quiet`
- `cargo fmt --check`
- `git diff --check`


---

<!-- event: state_changed author: hare at: 2026-07-18T12:49:12Z from: inprogress to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-18T12:49:12Z status: closed -->

## 完了

Worker stop/delete no longer cleans up Workdir materialization. Spawn failure rollback is limited to newly materialized Workdirs and uses Workdir cleanup by id so rollback does not leave corrupted records. Existing Workdir binds survive spawn failures.


---
