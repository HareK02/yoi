<!-- event: create author: tickets.sh at: 2026-06-01T02:11:04Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-05T23:01:38Z -->

## Decision

Updated based on user direction:

- keep this as the existing `tui-composer-history-persistence` ticket rather than creating a duplicate;
- default user-data shape should be like `~/.yoi/<path-to-pwd>/composer-history.json` using a path-safe/stable workspace key;
- do not create composer history under workspace `./.yoi/`;
- bound persisted recall history to about 30 entries per workspace instead of the older 100-entry note;
- keep typed `Segment` storage, non-destructive recall semantics, and no Pod/session transcript mutation.


---

<!-- event: intake_summary author: intake at: 2026-06-08T13:14:53Z -->

## Intake summary

既存 Ticket を確認し、同目的の open Ticket は他にないことを確認した。要件は、TUI composer の Up/Down recall 履歴を user data dir 配下に workspace-scoped で永続化し、workspace `./.yoi/`、Pod protocol、transcript、Worker/session history、model context には影響させないこと。保存対象は typed `Segment` vector、non-blank、連続重複抑止、workspace ごと最大 30 件程度。破損ファイルは startup を致命的に壊さず fallback する。受け入れ基準と validation は Ticket 本文に十分記録済みで、Orchestrator が routing 可能なため ready とする。

---

<!-- event: state_changed author: intake at: 2026-06-08T13:14:53Z from: planning to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake 確認により、既存 Ticket は実装 routing 可能な状態です。未解決の blocking open question はありません。主な reviewer focus は user-data persistence 境界、private/secret 混入前提の非露出、workspace 分離、既存 TUI recall semantics の回帰防止です。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T13:19:06Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-08T13:19:31Z -->

## Decision

Routing decision: defer_parallel_tui_work

This Ticket is queued and implementation-ready, but I am intentionally not accepting it to `inprogress` yet.

Reason:
- There is already an active queued-route TUI implementation in progress: `tui-chat-markdown-table-rendering`, with Coder Pod `coder-tui-chat-markdown-table-rendering` running in `.worktree/tui-chat-markdown-table-rendering`.
- `tui-composer-history-persistence` also touches TUI composer/input behavior and likely adjacent tests/helpers.
- Running both TUI changes concurrently would raise conflict/review risk without a clear need; this Ticket can follow immediately after the table-rendering Ticket reaches merge-ready or lands.

State decision:
- Leave this Ticket `queued`.
- Do not create a worktree or spawn a coder for it yet.

Next action:
- Re-read this Ticket after `tui-chat-markdown-table-rendering` finishes review/merge routing, then accept it from queued if still unblocked.

---
