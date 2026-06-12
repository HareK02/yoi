---
title: 'Panel Queue時にdevとOrchestrator worktreeを同期する'
state: 'inprogress'
created_at: '2026-06-12T01:16:39Z'
updated_at: '2026-06-12T08:45:59Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-06-12T02:39:25Z'
---

## 背景

Orchestrator 用 dedicated worktree を分離したことで、Panel が操作する root workspace と Orchestrator が Ticket tools で見る worktree は別 checkout になった。

```text
root workspace / dev
  Panel / human が Ticket を操作する場所

orchestration worktree / orchestration branch
  Orchestrator Pod の cwd
  Orchestrator の Ticket tools が見る `.yoi/tickets` backend
```

Ticket tools は Pod の cwd を基準に動作するべきであり、Orchestrator は orchestration worktree 側の `.yoi/tickets` を見る。そのため、Panel の Queue action で root/dev 側の Ticket を `ready -> queued` にするだけでは、Orchestrator が最新 Ticket state を見られない。

Queue は Orchestrator への durable handoff point なので、Orchestrator を kick する前に、root/dev 側の queued Ticket commit が orchestration worktree 側にも反映されている必要がある。

## ゴール

Panel の Queue action を、単なる Ticket state mutation ではなく、次の invariant を満たす安全な handoff として扱う。

1. root workspace の dev 側で対象 Ticket が `ready -> queued` になっている。
2. その変更が dev に commit されている。
3. orchestration worktree / branch がその commit を取り込んでいる。
4. Orchestrator は sync 済みの Ticket backend を見た状態で notify / restore / kick される。

## 要件

### Queue 実行前チェック

Panel の Queue action は、Ticket mutation 前に次の read-only check をすべて行う。ここに書いた条件以外を暗黙に判定しない。

- root workspace の canonical Git top-level が Panel の workspace root と一致する。
- orchestration worktree の canonical Git top-level が Panel が作成・記録した orchestration worktree path と一致する。
- root workspace と orchestration worktree の `git rev-parse --git-common-dir` が同じ repository を指す。
- root workspace の current branch が Panel の merge target branch（この repository では `develop`）と一致する。
- orchestration worktree の current branch が Panel の orchestration branch と一致する。
- root workspace の `git status --porcelain` が空である。
- orchestration worktree の `git status --porcelain` が空である。
- root workspace 側 Ticket backend で対象 Ticket が `ready` として読める。
- `git merge-base --is-ancestor <orchestration_head> <root_head>` が成功し、orchestration branch が root branch へ fast-forward 可能である。

いずれかが失敗した場合、Panel は Ticket mutation、commit、branch sync、Orchestrator notify/kick を行わず、失敗した check 名と対象 path / branch / Ticket id を表示する。

### Queue commit

Panel が Queue を実行する場合、root workspace 側で次の順序を守る。

1. 対象 Ticket を `ready -> queued` に遷移する。
2. `git status --porcelain` の差分が対象 Ticket record だけであることを確認する。
3. 対象 Ticket record だけを stage する。
4. dev 上に Queue commit を作る。
5. 作成した commit sha を記録する。

commit message は機械的でよいが、対象 Ticket id が分かるものにする。

Queue commit は Ticket handoff の authority であり、Orchestrator kick より前に完了している必要がある。

### dev -> orchestration sync

Queue commit 後、Panel は orchestration worktree を Queue commit へ fast-forward する。

自動同期で許可する操作は次だけに限定する。

```text
git -C <orchestration_worktree> merge --ff-only <queue_commit_sha>
```

この操作が失敗した場合、Panel は Queue を完了扱いにせず、Orchestrator notify/kick もしない。Panel は `orchestration branch cannot fast-forward to queue commit` と commit sha を表示する。

自動同期では merge commit、rebase、stash、patch apply、conflict resolution、dirty worktree cleanup を行わない。

### Orchestrator kick ordering

Orchestrator notify / restore / kick は、次を確認した後にだけ行う。

- root/dev 側の Queue commit sha が分かっている。
- orchestration worktree 側 HEAD がその commit を含んでいる。
- orchestration worktree 側の Ticket backend で対象 Ticket が `queued` として読める。

### Panel feedback

成功時は、Panel に次を表示する。

- queued Ticket id。
- dev 側 Queue commit sha。
- orchestration worktree sync 結果。
- Orchestrator notify/kick の有無。

失敗時は、どの条件で止まったかを具体的に表示する。

例:

- root workspace has uncommitted changes。
- orchestration worktree is dirty。
- orchestration branch cannot fast-forward to dev。
- orchestration worktree does not contain queued commit。

## 非目標

- dirty root workspace の変更を自動 stash / patch / commit すること。
- dirty orchestration worktree を自動修復すること。
- dev と orchestration branch の merge conflict を Panel が解決すること。
- Queue されていない Ticket を自動開始すること。
- Queue action で Orchestrator の routing / acceptance を代行すること。

## 受け入れ条件

- Panel Queue action が、Ticket `ready -> queued` を dev に commit してから orchestration worktree に同期する。
- sync 後、orchestration worktree 側 Ticket backend で対象 Ticket が `queued` として確認できる。
- Orchestrator notify / restore / kick は、orchestration worktree が Queue commit を含むことを確認してから行われる。
- root dirty / orchestration dirty / branch divergence / non-ff sync required の場合、Queue は block され、理由が Panel に表示される。
- Panel が自動 conflict resolution や stash を行わない。
- Ticket tools が cwd 基準で動く設計を前提に、workspace_root と cwd の分離を崩さない。
- `nix build .#yoi` が通る。
