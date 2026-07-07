---
title: 'Enable Browser-managed execution workspaces for Worker spawn'
state: 'inprogress'
created_at: '2026-07-07T12:22:06Z'
updated_at: '2026-07-07T13:41:43Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-07T13:40:23Z'
---

## 背景

Runtime 側には `CreateWorkerRequest.execution_workspace` と local Git worktree materializer があり、Worker 作成時に RepositoryPoint 相当の入力から detached worktree を materialize して Worker の workspace/cwd/scope にする境界が既にある。一方、Browser UI からは作業環境を事前に用意・確認し、それを Worker spawn 操作で選択する経路がまだ無い。

Browser から raw host path や内部 materialization request を直接指定させるのではなく、Workspace Backend が repository / selector / policy を検証して Execution Workspace を作成し、Browser には allocation id と安全な summary だけを返す形にする。

## 要件

- Browser-facing API で Execution Workspace を作成・一覧・詳細確認できる。
- v0 の materializer は既存方針どおり local Git repository からの detached worktree とし、remote clone/cache や dirty source inclusion は扱わない。
- Browser-facing payload に backend-private absolute path、raw materialization request、内部 runtime path を露出しない。
- Worker spawn form/API から、作成済み Execution Workspace を選択して Worker 作成に利用できる。
- Worker が利用する cwd 指定は host absolute path ではなく、materialized workspace root からの安全な `relative_cwd` として扱う。
- `relative_cwd` は absolute path、`..` escape、symlink escape、存在しない/不正な working directory を拒否する。
- Worker detail / list には execution workspace の safe summary を表示する。
- cleanup policy と手動 cleanup 操作の v0 方針を明示する。

## 受け入れ条件

- `POST /api/w/<workspace-id>/execution-workspaces` 相当の API で configured repository と selector から Execution Workspace を作成できる。
- `GET /api/w/<workspace-id>/execution-workspaces` 相当の API で作成済み Execution Workspace の safe summary を一覧できる。
- Worker launch UI で Execution Workspace を選択でき、spawn request が raw path ではなく safe id/selector 情報を送る。
- Worker spawn 時に選択した Execution Workspace が Runtime の `CreateWorkerRequest.execution_workspace` または同等の backend-resolved request に接続される。
- 作成された Worker の workspace/cwd/scope が materialized worktree 配下になり、Browser/API には内部 path が漏れない。
- 不正な repository、dirty unsupported source、remote unsupported source、invalid `relative_cwd` は typed diagnostic として拒否される。
- frontend check/test、workspace-server tests、`yoi ticket doctor`、`nix build .#yoi` が通る。

## 設計メモ

初期実装は pre-allocate 方式を優先する。つまり Browser で Execution Workspace を明示的に作成し、Worker spawn では `allocation_id` と任意の `relative_cwd` を選択する。単発 spawn request だけで暗黙 materialize する経路は、UI/cleanup/diagnostics が固まるまで後回しにする。
