<!-- event: create author: "yoi ticket" at: 2026-07-18T03:17:55Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-18T03:18:16Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T03:18:16Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T03:18:16Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T03:18:16Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-18T03:20:37Z -->

## Implementation report

`script/dev-workspace.sh` ではなく repository root の `scripts/dev-workspace.sh` として開発 process 管理 script を追加した。

実装内容:
- `start` / `stop` / `restart` / `status` action を追加。
- `start` は runtime, backend, frontend をこの checkout から起動する。
- `stop` は runtime, backend, frontend を止める。
- `restart` は frontend を触らず、runtime/backend だけ止めて起動し直す。
- frontend は既定で `0.0.0.0:5173` bind。
- backend は既定で `127.0.0.1:8787`、runtime は `127.0.0.1:38800`。
- pid/log は `.yoi/dev/{pids,logs}` に保存し、`.gitignore` に `/.yoi/dev` を追加。
- 既存 listener がいる場合、port listener PID を止めてからこの checkout の process を起動する。
- managed process は pidfile の pid/process group を使って止める。pidfile 管理外の既存 listener は、caller の process group を巻き込まないよう PID 単位で止める。

確認:
- `bash -n scripts/dev-workspace.sh`
- `scripts/dev-workspace.sh --help`
- `scripts/dev-workspace.sh status`
- `git diff --check`

実行しなかったこと:
- `scripts/dev-workspace.sh start`
- `scripts/dev-workspace.sh stop`
- `scripts/dev-workspace.sh restart`

理由:
- 現在の worker 自体が既存 backend/runtime/frontend process 群に依存している可能性があり、停止・再起動するとこのセッションが落ちるため。

現状確認:
- frontend listener pid は `/home/hare/Projects/yoi/web/workspace` 由来。
- backend/runtime listener pid は `/home/hare/Projects/yoi` 由来。


---

<!-- event: state_changed author: hare at: 2026-07-18T08:32:27Z from: inprogress to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-18T08:32:27Z status: closed -->

## 完了

Added `scripts/dev-workspace.sh` with start/stop/restart/status. Start moves runtime/backend/frontend to this checkout, frontend binds `0.0.0.0`, restart intentionally leaves frontend untouched. Runtime files are kept under ignored `.yoi/dev/`.


---
