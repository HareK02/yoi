---
title: 'Workspace Orchestrator-managed workdirs and Worker spawn tools'
state: 'planning'
created_at: '2026-07-15T16:28:09Z'
updated_at: '2026-07-15T16:30:00Z'
assignee: null
---

## 背景

現在の LLM-facing `SpawnWorker` は child Worker の process/tool `cwd`、profile、initial input、delegation scope を扱うが、Workspace Runtime の `WorkingDirectory` 作成・claim・Worker 紐付け・cleanup authority は扱わない。

一方で workspace-server / worker-runtime には `WorkingDirectoryRequest`、`WorkingDirectoryClaim`、browser Worker create API の workdir selection、workdir registry / worker link が既にある。embedded Orchestrator が coder / reviewer などの sibling Worker 群を律するには、raw path / cwd ではなく Workspace backend-managed workdir authority を通して Worker spawn できる tool surface が必要。

## 要件

- 既存 `SpawnWorker` を膨らませず、Workspace backend API 経由で workdir と Worker spawn を扱う Orchestrator 用 tool feature を追加する。
- Orchestrator tool は `WorkspaceClient::Http { workspace_id, base_url }` を必須 authority とし、無い場合は fail closed / not registered にする。
- 最小 tool surface として以下を提供する。
  - Workspace-managed working directory の一覧または詳細取得。
  - repository id / selector から working directory を作成する操作。
  - existing `working_directory_id` / optional `relative_cwd` を指定して Worker を spawn する操作。
- Worker spawn 操作は Workspace backend の safe request schema を使い、raw filesystem path、runtime-internal store path、executable path、socket path、secret-like value を model input として受け取らない。
- Spawn する Worker には profile selector、display/requested name、initial input、acceptance requirement を指定できる。
- Workspace backend は spawn 結果として Worker id、runtime id、console href、working directory summary、diagnostics を返し、workdir registry / worker-workdir link を更新する。
- Orchestrator role/profile だけに mutation-capable tools を出し、Coder / Reviewer などには原則として sibling spawn / workdir create 権限を出さない。
- Ticket orchestration flow からは、Ticket を読んだ後に workdir create/select -> coder/reviewer spawn -> Worker status/output 追跡へ進める形にする。
- `SpawnWorker.cwd` と Workspace `WorkingDirectory` の違いを prompt/tool description で明確にし、cwd は authority ではないことを維持する。

## 受け入れ条件

- embedded Orchestrator Worker から Workspace API 経由で workdir を作成できる。
- embedded Orchestrator Worker から作成済み workdir を指定して sibling Worker を spawn できる。
- Spawn された Worker が Runtime / Workspace backend の worker list と workdir link に現れる。
- raw path を渡す spawn/workdir tool input は schema または validation で拒否される。
- `WorkspaceClient::Http` を持たない Worker では Orchestrator workspace mutation tools が登録されない、または実行時に fail closed する。
- 既存 `SpawnWorker` の local delegation behavior は壊れない。
- worker-runtime / workspace-server / worker の該当 tests と `nix build .#yoi` が通る。
