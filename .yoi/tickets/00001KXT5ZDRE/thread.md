<!-- event: create author: "yoi ticket" at: 2026-07-18T08:37:37Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-18T08:38:38Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T08:38:38Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T08:38:38Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T08:38:38Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-18T08:40:59Z -->

## Implementation report

`scripts/dev-workspace.sh` の mutating actions を既定で detached scheduled job として実行するように変更した。

変更点:
- `start` / `stop` / `restart` は通常呼び出しでは実処理をせず、detached job を schedule して即時 return する。
- 既定 delay は `YOI_DEV_ACTION_DELAY_SECONDS=60`。
- 呼び出し元には `scheduled_action` / `scheduled_pid` / `scheduled_after_seconds` / `scheduled_log` を表示する。
- scheduled job は `YOI_DEV_WORKSPACE_FOREGROUND=1` を付けて同じ script を再入実行し、そこで実際の start/stop/restart を行う。
- `status` / `help` は同期実行のまま。
- デバッグ用に `YOI_DEV_WORKSPACE_FOREGROUND=1` で foreground 実行できるようにした。
- help に detached 既定挙動と環境変数 override を追記した。

検証:
- `bash -n scripts/dev-workspace.sh`
- `scripts/dev-workspace.sh --help >/dev/null`
- `scripts/dev-workspace.sh status >/dev/null`
- `git diff --check`

実行しなかったこと:
- `scripts/dev-workspace.sh start`
- `scripts/dev-workspace.sh stop`
- `scripts/dev-workspace.sh restart`

理由:
- mutating action は schedule 後に backend/runtime/frontend を止めるため、この作業中には実行しない。


---

<!-- event: state_changed author: hare at: 2026-07-18T08:40:59Z from: inprogress to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-18T08:40:59Z status: closed -->

## 完了

`scripts/dev-workspace.sh` now schedules `start`, `stop`, and `restart` as detached background jobs by default, returning schedule/log details immediately. `status` and `help` remain synchronous, and `YOI_DEV_WORKSPACE_FOREGROUND=1` preserves an explicit foreground/debug path.


---
