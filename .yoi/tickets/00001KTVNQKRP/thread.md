<!-- event: create author: "yoi ticket" at: 2026-06-11T15:45:07Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-11T15:45:52Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-11T15:45:52Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-11T15:45:53Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-11T15:45:53Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-06-11T15:59:55Z -->

## Implementation report

runtime workspace と process cwd を分離する実装を行った。

- `--workspace` は runtime workspace / project context の基準として維持し、child process の current directory とは分離した。
- `SpawnConfig` / Ticket role launch plan は内部値として `cwd` を持ち、CLI argument にはせず `Command::current_dir` にのみ使うようにした。
- Panel Orchestrator launch/restore は `workspace_root = original workspace`、`cwd = dedicated orchestration worktree` として起動する。
- `SpawnPod.cwd` は child process cwd 指定として扱い、`--workspace` は spawner の runtime workspace を渡すままにした。
- Pod entrypoint は process cwd を起動時に snapshot し、workspace root とは別に Pod construction へ渡す。`--tool-cwd` は作らない。
- `pwd` と `cwd` の意図的な意味差がない主要 runtime path は `cwd` に統一した。
- metadata snapshot restore では、現在の launch manifest の scope/delegation scope を復元後 manifest に反映できるようにした。

検証:

- `cargo check`
- `cargo test -p client`
- `cargo test -p pod entrypoint::tests:: -- --nocapture`
- `cargo test -p pod spawn_pod_launches_runtime_in_workspace_and_process_cwd -- --nocapture`
- `cargo test -p pod spawn_pod_omitted_cwd_preserves_spawner_cwd -- --nocapture`
- `cargo fmt --check`
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi`

補足:

- `cargo test -p tools` は doctest の既存不整合 (`core_builtin_tools` の引数数) で失敗したため、この実装の検証対象からは外した。通常 unit/integration tests は通っている。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-11T15:59:55Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-06-13T16:34:06Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-13T16:34:06Z status: closed -->

## 完了

Closed after prior done-state completion.


---
