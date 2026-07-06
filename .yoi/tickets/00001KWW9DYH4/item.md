---
title: 'Add local Git worktree Execution Workspace materializer'
state: 'ready'
created_at: '2026-07-06T18:00:46Z'
updated_at: '2026-07-06T18:26:52Z'
assignee: null
---

## 背景

現在の Runtime は `--workspace` で与えられた単一 root を Worker の workspace scope として使っている。そのため、同じ repository root を使う Worker を複数 spawn すると worker allocation conflict が起きる。また、Runtime が source repository root を直接 Worker に渡しており、Execution Workspace materialization / cleanup / evidence / sandbox の境界がまだない。

Objective `00001KWW44EXK` では、Runtime が RepositoryPoint から Worker ごとの Execution Workspace を materialize し、Worker は source repository root ではなく materialized workspace を scope として起動する方針を定めた。この Ticket ではその v0 として、remote/cache-backed Git clone は扱わず、Repository `uri` が local Git repository を指し、そこから `git worktree add --detach` できる前提で最小の materializer を実装する。

ローカル origin の Git clone / Runtime repository-cache / bare mirror cache はこの Ticket では非目標とする。v0 は local source repository 自体を source cache 相当として扱い、Runtime root 配下に Worker ごとの detached worktree を作る。

## 要件

- Runtime は Worker spawn 前に Execution Workspace materializer を呼ぶ境界を持つ。
- v0 materializer は configured local Git repository から detached worktree を Runtime root 配下に作る。
- Worker は source repository root ではなく、materialized worktree path を workspace scope / cwd として起動する。
- `RepositoryId` / `RepositorySelector` / resolved `RepositoryPoint` の将来境界を壊さない型にする。
- この Ticket では local URI の Git repository のみ対応する。remote URI、bare mirror cache、clone/fetch、credential resolution は扱わない。
- worktree は branch 名ではなく resolved commit に対する detached worktree として作る。v0 では branch を作らず、Worker の変更は detached worktree 上の diff / patch artifact として回収する。branch 作成は後続の merge/export policy として扱う。
- dirty local changes は暗黙に含めない。v0 は `clean_point_only` とし、必要なら dirty state diagnostic を返す。
- Execution Workspace は `.yoi` や Backend workspace store ではなく Runtime root 配下に作る。
- Worker stop / cleanup 時に worktree cleanup できる方針または最低限の cleanup record を持つ。
- Browser-facing/API-facing diagnostics は raw host path を不必要に漏らさない。

## 受け入れ条件

- `CreateWorkerRequest` または Runtime-side spawn path に Execution Workspace materialization request / allocation 境界が追加されている。
- local Git repository `uri = "."` 相当から resolved commit を取得し、Runtime root 配下の `execution-workspaces/<allocation-id>/root/<repository-id>` に `git worktree add --detach` できる。
- Worker 用の Git branch は v0 materialization では作成されず、変更回収は detached worktree の diff / patch artifact として扱われる。
- Worker spawn 時、Worker scope/cwd は source repository root ではなく materialized worktree を指す。
- 同じ local Git repository を対象に複数 Worker を spawn しても、source root scope の allocation conflict で失敗しない。
- created allocation には repository id、resolved commit/tree などの evidence に使える情報、materializer kind、cleanup target が記録される。
- unsupported non-local URI / non-Git provider は typed diagnostic で拒否される。
- dirty local workspace を暗黙に含めず、v0 policy として clean point only であることが test で分かる。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cd web/workspace && deno task check && deno task test` が通る。

## 非目標

- remote Git URI の clone/fetch/cache を実装すること。
- local URI から Runtime repository-cache / bare mirror cache を作ること。
- CoW snapshot / Rift-like snapshot / reflink / overlay filesystem を実装すること。
- 強い container sandbox / network policy / secret distribution を完成させること。
- dirty working tree snapshot / patch artifact apply を実装すること。
