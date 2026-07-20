<!-- event: create author: "yoi ticket" at: 2026-07-18T10:09:26Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-18T10:09:33Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T10:09:33Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T10:09:33Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T10:09:33Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-18T10:10:43Z -->

## Implementation report

`scripts/dev-workspace.sh` の先頭に使い方と注意コメントを追加した。

追記内容:
- `status` / `restart` / `start` / `stop` の基本用途。
- `start` / `stop` / `restart` は既定で detached schedule されること。
- `restart` は backend/runtime のみで frontend を触らないこと。
- frontend を worktree 間で移す場合は `start` / `stop` を使うこと。
- `YOI_DEV_WORKSPACE_FOREGROUND=1` は active API session 中に避けるべきこと。
- detached action 後は `scheduled_log` を確認すること。

検証:
- `bash -n scripts/dev-workspace.sh`
- `scripts/dev-workspace.sh --help >/dev/null`
- `scripts/dev-workspace.sh status >/dev/null`
- `git diff --check`

mutating action (`start`/`stop`/`restart`) は実行していない。


---

<!-- event: state_changed author: hare at: 2026-07-18T10:10:43Z from: inprogress to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-18T10:10:43Z status: closed -->

## 完了

Added top-of-file usage and safety comments to `scripts/dev-workspace.sh`, documenting detached default behavior, restart/frontend semantics, foreground override risk, and scheduled log follow-up.


---
