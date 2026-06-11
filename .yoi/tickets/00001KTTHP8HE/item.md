---
title: 'Panel Orchestrator起動時に専用orchestration worktreeを自動作成・再利用する'
state: 'inprogress'
created_at: '2026-06-11T05:15:14Z'
updated_at: '2026-06-11T08:12:54Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-11T07:41:37Z'
---

## 背景

`00001KTTB479X` で、Orchestrator の runtime workspace / Ticket backend root と、implementation worktree を作る original workspace root、および merge target workspace root を分離する 4-root model が導入された。

```text
role_workspace_root
original_workspace_root
implementation_worktree_root
merge_target_workspace_root
```

ただし、現時点では Panel / workspace orchestration が実際に dedicated orchestration worktree を作成し、その worktree を `role_workspace_root` として Orchestrator を起動する経路は follow-up 境界として残っている。通常起動では多くの場合、`role_workspace_root == original_workspace_root == merge_target_workspace_root` のままである。

filesystem Ticket backend では `.yoi/tickets` が Git 管理ファイルであるため、ユーザーの main workspace と Orchestrator が同じ worktree / branch の Ticket record を同時に触ると、draft / queue / orchestration progress / project record / Git dirty state が混ざりやすい。Orchestrator を専用 orchestration worktree で実行し、main workspace の未投入 draft と Orchestrator の active queue / progress を分離する必要がある。

## ゴール

Workspace Panel から Orchestrator を起動・復元する際に、original workspace root 配下の `.worktree` に専用 orchestration worktree を自動作成または再利用し、その worktree を Orchestrator Pod の `workspace_root` / `role_workspace_root` として使う。

Implementation worktree は引き続き original workspace root の `.worktree/<ticket-or-task>` に sibling として作成し、orchestration worktree の子や相対 path にはしない。

## 要件

- Panel / workspace Orchestrator lifecycle に dedicated orchestration worktree launch path を追加する。
- Orchestrator 起動時の root を明確に分ける。
  - `role_workspace_root`: dedicated orchestration worktree。
  - `original_workspace_root`: Panel を開いた元 workspace root。
  - `implementation_worktree_root`: `<original_workspace_root>/.worktree`。
  - `merge_target_workspace_root`: default は original workspace root。将来 configured target branch/workspace を許容できる設計にする。
- orchestration worktree の path / branch naming policy を定義する。
  - 例: `<original_workspace_root>/.worktree/orchestration/<workspace-or-session-slug>`。
  - branch 例: `orchestration/<workspace-or-session-slug>`。
  - path/branch 名は collision-safe かつ workspace ごとに安定する。
- 既存 orchestration worktree がある場合の reuse / attach / fail policy を定義する。
  - clean and valid なら reuse する。
  - dirty / missing branch / inconsistent Git metadata / live conflicting Pod がある場合は安全に診断し、破壊的 cleanup はしない。
- orchestration worktree 作成は explicit Panel Orchestrator lifecycle の一部として扱い、通常の implementation worktree 作成とは区別する。
- Orchestrator role launch では `TicketRoleLaunchContext` に distinct roots を populate する。
  - `workspace_root` は orchestration worktree。
  - `original_workspace_root` は元 workspace root。
  - `target_workspace_root` は merge target workspace root。
- `SpawnConfig.workspace_root` は Orchestrator の runtime workspace / Ticket backend root として orchestration worktree を指す。
- Orchestrator prompt の `Workspace routing context` に distinct roots が出ることを維持する。
- main workspace の未投入 Ticket draft / local planning は、明示的に orchestration worktree へ取り込むまで Orchestrator queue として扱わない。
- implementation worktree は orchestration worktree から派生させず、original workspace root の `.worktree/<ticket-or-task>` に作る guidance / launch context を維持する。
- merge-completion は process cwd ではなく recorded `merge_target_workspace_root` で実行する guidance を維持する。
- cleanup 方針を最小限定義する。
  - orchestration worktree の自動削除はこの Ticket では慎重に扱い、少なくとも dirty/unknown worktree を自動削除しない。
  - implementation worktree cleanup とは別物として扱う。
- UI / diagnostics で、Panel が見ている Orchestrator / Ticket backend root が main workspace ではなく orchestration worktree であることを確認できるようにする。
- Git 操作は安全側に倒す。
  - `git worktree add` / branch 作成前に existing path/branch/worktree を確認する。
  - unrelated dirty state を破壊しない。
  - push はしない。

## 受け入れ条件

- Ticket-enabled workspace で Panel が Orchestrator を spawn する際、dedicated orchestration worktree が作成または再利用される。
- Orchestrator Pod の `workspace_root` / runtime cwd / Ticket backend root が orchestration worktree になる。
- Orchestrator launch prompt に distinct root context が含まれる。
  - `role_workspace_root` = orchestration worktree。
  - `original_workspace_root` = 元 workspace root。
  - `implementation_worktree_root` = `<original_workspace_root>/.worktree`。
  - `merge_target_workspace_root` = target workspace root。
- Orchestrator が implementation worktree を作る際の guidance は original workspace root 配下を指す。
- Orchestrator が merge/validation/cleanup する際の guidance は recorded merge target workspace を指す。
- 既存 orchestration worktree を安全に reuse できる。
- dirty / inconsistent / conflicting orchestration worktree がある場合、Panel は分かる診断を出し、破壊的 cleanup や上書きをしない。
- main workspace の Ticket draft が、単に main workspace に存在するだけでは dedicated orchestration worktree の Orchestrator queue にならない運用境界が維持される。
- tests が追加・更新されている。
  - orchestration worktree path/branch naming。
  - existing worktree reuse / unsafe state diagnostics。
  - `TicketRoleLaunchContext` roots population。
  - `SpawnConfig.workspace_root` が orchestration worktree を指すこと。
  - prompt に root context が出ること。
- `cargo test -p tui` または targeted TUI/client tests、`cargo fmt --check`、`git diff --check`、`target/debug/yoi ticket doctor` が通る。

## 非目標

- Ticket backend を Git 外 store に移行すること。
- implementation worktree の host-side full automation を再設計すること。
- Orchestrator が push すること。
- orchestration worktree の破壊的自動 cleanup を導入すること。
- main workspace への project record publication policy を完全に解くこと。
- Git worktree を使わない project 向けの別 storage backend をこの Ticket で設計すること。

## 関連

- `00001KTTB479X`: 4-root model と prompt/workflow guidance。dedicated Orchestrator worktree の実起動は follow-up 境界。
- `00001KTG3AZQ8`: implementation worktree + coder/reviewer routing。host-side Git automation ではなく Orchestrator guidance ベース。
