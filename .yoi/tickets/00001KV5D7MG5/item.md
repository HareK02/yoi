---
title: 'Panel に orchestration worktree の Ticket state overlay を表示する'
state: 'closed'
created_at: '2026-06-15T10:29:00Z'
updated_at: '2026-06-15T14:59:40Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['panel', 'ticket-state', 'orchestration', 'worktree', 'git-branch', 'read-only-overlay']
queued_by: 'workspace-panel'
queued_at: '2026-06-15T12:39:21Z'
---

## Background

Workspace Panel は現在、開いている workspace / branch の `.yoi/tickets` を Ticket state の authority として表示している。これは branch-local Ticket 正本として正しいが、Orchestrator が dedicated orchestration worktree / branch 上で Ticket を `inprogress -> done` に進めた場合、current workspace branch 側には merge されるまで反映されない。

その結果、Panel では `queued` のままに見える一方、orchestration branch では `inprogress` や `done` まで進んでいる、という乖離が起きる。

この Ticket では current branch の Ticket state を上書きせず、configured orchestration worktree の Ticket state を read-only overlay として読み、Panel に source-qualified progress として表示する。

## Requirements

- Panel の primary Ticket state は current workspace branch の `.yoi/tickets` のままにする。
- Configured orchestration worktree / branch を read-only overlay source として読む。
  - `.yoi/ticket.config.toml` の `[orchestration]` 設定を使う。
  - `branch`
  - `worktree_dir`
  - `worktree_name`
- Expected orchestration worktree path を current workspace root から解決する。
  - default は `<workspace>/.worktree/orchestration`
  - branch default は `orchestration`
- Overlay worktree を読む前に safety checks を行う。
  - path が存在する
  - git worktree である
  - current repository と同じ git common dir / same repository である
  - expected branch 上にいる
  - canonical top-level が expected path と一致する
- Overlay は read-only とする。
  - Panel が overlay Ticket file を変更しない。
  - overlay state を current branch の Ticket file に自動反映しない。
  - current branch の `state` を orchestration branch の `state` で上書きしない。
- Ticket identity は canonical Ticket id で join する。
  - current branch に存在する Ticket に対して、同じ id の overlay Ticket があれば overlay state を表示する。
  - overlay にしか存在しない Ticket の表示は実装時に方針を決めてよいが、current workspace Ticket list を勝手に増やす場合は source 表示を明確にする。
- Overlay 表示は source-qualified にする。
  - 例: `orchestration: inprogress`
  - 例: `orchestration: done · merge pending`
  - 例: `local: queued · orchestration: done`
- Overlay state が current state より進んでいる場合、Panel action/gate は safety 側に倒す。
  - current branch が `queued` でも overlay が `inprogress` / `done` なら、Queue/Start を再提示しない。
  - overlay `done` なら merge/review/close pending として扱う導線を表示する。
  - exact action wording は実装側で調整してよい。
- Overlay が読めない場合は、current branch state 表示にフォールバックする。
  - branch mismatch / invalid worktree / missing worktree は bounded diagnostic として表示してよい。
  - 誤った worktree の Ticket state を表示しない。

## Display guidance

Panel Ticket row は current branch の canonical state と overlay progress を分けて表示する。

例:

```text
queued  E2E: close remaining critical-path gaps after panel harness
00001KV10SN02 | local: queued · orchestration: done · merge pending
```

または:

```text
ready  Extend pod::feature API...
00001KTR81P9X | orchestration: inprogress · yoi-orchestrator
```

重要なのは、line 1 の canonical state を current branch state として残し、line 2 / gate / detail に orchestration overlay を表示すること。

## Acceptance criteria

- Workspace Panel ViewModel が configured orchestration worktree の `.yoi/tickets` を read-only overlay として読み込める。
- Overlay source は expected path / branch / same repository safety checks を通過した場合のみ使われる。
- Current branch の Ticket state は overlay state で上書きされない。
- current branch `queued` + overlay `inprogress` の Ticket が、Panel で overlay progress として表示される。
- current branch `queued` + overlay `done` の Ticket が、Panel で `orchestration: done` / merge pending 相当として表示され、Queue/Start action を再提示しない。
- overlay unavailable / branch mismatch / missing worktree の場合、Panel は panic せず current branch 表示にフォールバックする。
- Tests cover:
  - overlay join by Ticket id
  - queued local + inprogress overlay display
  - queued local + done overlay display/action gating
  - overlay branch mismatch ignored
  - missing overlay worktree fallback
  - no mutation to current branch Ticket files
- Validation: focused `tui` / workspace panel tests, `cargo fmt --check` or `cargo fmt -p tui`, `cargo check -p tui --all-targets`, and `git diff --check`.

## Non-goals

- Automatically merging orchestration branch state into current branch.
- Closing current branch Tickets based on overlay state.
- Starting/stopping Orchestrator Pods.
- Changing Ticket lifecycle state schema.
- Replacing git merge/review evidence with runtime overlay state.
- Showing arbitrary unrelated worktree Ticket state.

## Related work

- Orchestration worktree branch/path configuration: `00001KV0X254D`
- Panel two-line Ticket rows and gate display: `00001KV12W2RT`
- Panel visual hierarchy follow-up: `00001KV4ZPAD3`
