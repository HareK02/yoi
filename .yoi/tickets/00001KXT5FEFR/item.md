---
title: 'Limit workdir dirty source check to HEAD selector'
state: 'closed'
created_at: '2026-07-18T08:28:54Z'
updated_at: '2026-07-18T08:32:27Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-18T08:29:31Z'
---

## 背景

Runtime workdir materializer は `selector` を `git rev-parse <selector>^{commit}` で commit に解決し、`git worktree add --detach` で detached worktree を作る。明示 branch / tag / commit selector の場合、source worktree の未コミット変更は生成される detached worktree には入らないため、dirty source を理由に拒否する必要はない。

一方で `HEAD` selector は現在の source worktree の状態を暗黙に参照する導線なので、既存の dirty source safety check を維持する。

## 要件

- workdir materialization の dirty source check は実質 selector が `HEAD` の時だけ行う。
- 明示 branch selector では source repository が dirty でも commit 解決と detached worktree 作成を許す。
- `HEAD` selector の dirty source rejection は維持する。

## 受け入れ条件

- `selector = HEAD` で source repository が dirty の場合、`working_directory_dirty_source_rejected` になる。
- `selector = <branch>` で source repository が dirty の場合、workdir materialization が成功する。
- 明示 branch selector で作った workdir は dirty source の未コミットファイルを含まない。
- focused worker-runtime tests が通る。
