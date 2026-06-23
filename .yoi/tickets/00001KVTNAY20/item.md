---
title: 'Abstract Workspace Worker runtime spawn operations'
state: 'ready'
created_at: '2026-06-23T16:34:39Z'
updated_at: '2026-06-23T18:26:48Z'
assignee: null
---

## 背景

Workspace web / Workspace backend から Worker を起動・停止・操作する UI/API を作るには、現在 read-only live view として実装されている runtime bridge を、Worker 操作も扱える backend abstraction に拡張する必要がある。

現状:

- `crates/workspace-server/src/hosts.rs` の `LocalRuntimeBridge` は local runtime metadata を読んで `HostSummary` / `WorkerSummary` を返す read-only bridge である。
- `/api/hosts`、`/api/workers`、`/api/hosts/{host_id}/workers` はこの bridge を直接使っている。
- Worker 起動の最終境界は `yoi pod [POD_OPTIONS]` process launch である。
- 既存の `client::spawn::SpawnConfig` は `yoi pod` 起動 options の typed 表現に近いが、`ticket_role` のような Ticket/Orchestration domain の情報も混ざっている。
- Workspace backend には Worker spawn / stop / protocol proxy の抽象境界がまだない。
- Worker 起動を web backend から実装する際、workspace root / cwd / pod name / profile / role-session claim の経路が TUI とズレると、Panel 表示や workspace 所属判定が壊れる。

この Ticket では、Workspace backend から Worker spawn 操作を実装する前段として、**Pod process 起動境界**と**Ticket / Orchestration intent 境界**を分離して整理する。低レベル起動は Ticket system を知らず、Ticket/Orchestration 情報は上位 resolver / launch policy が低レベル config へ落とす。

## 要件

### Layering / responsibility split

起動系は少なくとも以下の層に分ける。

1. **Pod process launch layer**
   - `yoi pod [POD_OPTIONS]` を起動する唯一の低レベル境界。
   - Ticket / Orchestration / Role / TicketId を直接知らない。
   - `workspace_root`、`pod_name`、`profile`、`manifest`、`project`、`store`、`cwd`、`initial_input`、`workflow`、`require_pod_state` のような process 起動 options だけを扱う。
2. **Worker runtime layer**
   - Workspace backend / API から見える Worker 操作境界。
   - hosts/workers live view、worker lookup、spawn/stop/protocol proxy の entrypoint を持つ。
   - API intent を低レベル Pod process launch config へ解決する。
3. **Ticket / Orchestration resolver layer**
   - Ticket ID、Ticket role、Orchestrator/Coder/Reviewer といった workflow domain を扱う。
   - Role/Profile/Workflow/Ticket context を解決し、低レベル Pod process launch config と必要な launch policy / prompt input を作る。
   - Ticket/Orchestration 情報を低レベル launcher に直接漏らさない。

### Pod process launch abstraction

- `SpawnConfig` をそのまま Workspace backend API の入力として扱わない。
- 低レベルの起動 config は Ticket/Orchestration domain を持たない形に整理する。
  - 既存 `SpawnConfig` を改名/分割するか、別の `PodProcessLaunchConfig` / `PodLaunchOptions` を導入する。
- `ticket_role` は低レベル process config から分離する。
  - 必要なら `PodLaunchPolicy` / `ProfileLaunchPolicy` のような別引数/別 layer に移す。
  - `--ticket-role` は現状 hidden CLI marker として存在してよいが、汎用 `yoi pod` 起動 config の概念には含めない。
- 低レベル launcher は `Command::new("yoi")` 相当の process 起動と acceptance evidence 取得に責務を限定する。
- TUI/Panel、SpawnPod tool、Workspace backend はこの低レベル launcher を共有できるようにする。

### Worker runtime abstraction

- `LocalRuntimeBridge` 相当の read-only live view を、Worker 操作用の抽象境界へ整理する。
- trait 名は実装時に決めてよいが、概念としては `WorkerRuntime` / `WorkspaceWorkerRuntime` / `HostRuntime` のような境界を作る。
- 最低限、以下の責務を同じ abstraction から扱えるようにする。
  - hosts list
  - workers list
  - worker detail / lookup
  - worker spawn request
  - worker stop request
  - 将来の protocol method proxy / event stream の接続点
- v0 では local implementation のみでよい。
  - 例: `LocalPodRuntime` / `LocalWorkspaceRuntime`。
- 将来 Host protocol / remote worker backend を差し替えられるよう、Workspace API handler が concrete local implementation に密結合しすぎない形にする。

### Spawn operation abstraction

- Backend API から Worker を起動するための typed request / result を設計する。
- Browser から `workspace_root` / `cwd` / executable path / raw profile selector を自由入力させない。
- Frontend/API request は intent を中心にする。
  - 例: generic worker profile launch、orchestrator start、ticket coder/reviewer start。
  - ticket_id が必要な operation は path または typed field から受ける。
- Backend / resolver が以下を解決する。
  - canonical workspace root
  - process cwd
  - pod name
  - profile / manifest / project
  - initial input / workflow
  - optional launch policy
  - role-session claim
- Orchestrator の dedicated worktree 起動では、runtime workspace identity と cwd を混同しない。
  - `workspace_root` は original/main workspace
  - `cwd` は orchestration worktree
- Coder / Reviewer / Orchestrator の Ticket-specific 起動経路は、低レベル Pod process launch config を生成する上位 resolver として扱う。
- Workspace backend が独自に `Command::new("yoi")` を組み立てて挙動を分岐させない。

### Ticket / Orchestration information boundary

- Ticket ID、Ticket role、Orchestration role は低レベル `PodProcessLaunchConfig` に入れない。
- Ticket/Orchestration resolver は以下を出力する。
  - low-level Pod process launch config
  - optional profile/launch policy
  - role-session claim metadata
  - model-visible initial input / workflow selection
- Role は Profile と同一視しない。
  - Role は workflow 上の責務。
  - Profile は Pod recipe。
  - Role/Profile/Ticket の対応は上位 resolver の責務であり、低レベル launcher の責務ではない。
- Role を使わない generic Worker 起動では Ticket/Orchestration 情報を一切要求しない。

### API surface planning

- 実装する API はこの Ticket で確定しなくてもよいが、初期案として以下を検討する。
  - generic: `POST /api/workers/spawn`
  - operation: `POST /api/workers/{worker_id}/stop`
  - orchestration: `POST /api/orchestrator/start`
  - ticket-specific: `POST /api/tickets/{ticket_id}/workers`
- 起動 operation は HTTP command として扱う。
- 状態観測 / stream / transcript は後続 Ticket で SSE/WebSocket を検討する。
- spawn success は process start だけでなく、socket/connect/snapshot 等の acceptance evidence を持つこと。

### Role-session / Panel integration

- Backend spawn でも Panel が Worker を見つけられるようにする。
- 起動成功後に必要な local role-session claim を作る、または既存 helper を共有する。
- Runtime registry、Pod metadata、role-session registry のどれを authority / hint として扱うかを明確にする。
- stale/missing/already-running の扱いを typed error として返す。

### Safety / authority boundary

- Browser は local Unix socket path や runtime registry path を直接知らない。
- Backend が worker identity を解決し、対象 Worker が current Workspace から見えることを確認する。
- user permission model はこの Ticket の scope 外だが、将来挟める場所を backend abstraction に残す。
- Raw Pod protocol を browser から受ける場合でも、backend が最低限の method allow/block を挟める構造にする。

## Non-goals

- Worker operation UI の完成。
- WebSocket/SSE event stream の完成。
- Full permission / user auth model の設計。
- Remote Host protocol implementation。
- Pod protocol TypeScript 型生成の実装。
- Workspace identity `.yoi/workspace.toml` 実装。
- StopPod / registry locking の追加修正。
- Ticket/Orchestration domain を廃止すること。
- `--ticket-role` hidden CLI marker の即時削除。

## 受け入れ条件

この Ticket は planning から開始する。ready に進める前に以下を満たす。

- Workspace backend runtime abstraction の責務境界が明文化されている。
- `yoi pod [POD_OPTIONS]` process launch 境界と Ticket/Orchestration resolver 境界が分離されている。
- 低レベル Pod process launch config が Ticket ID / Ticket role / Orchestration role を直接持たない方針が明記されている。
- 既存 `SpawnConfig` の `ticket_role` をどう分離するかが決まっている。
- 現在の `LocalRuntimeBridge` をどう trait / service に置き換えるかが決まっている。
- Worker spawn request / result の typed shape が設計されている。
- Orchestrator / Coder / Reviewer の起動時に `workspace_root` と `cwd` をどう決めるかが明記されている。
- TUI/Panel、SpawnPod tool、Workspace backend が低レベル Pod process launcher を共有する方針が明記されている。
- Ticket/Orchestration resolver が低レベル launch config・launch policy・role-session claim を生成する方針が明記されている。
- Backend API endpoint の初期案が整理されている。
- role-session claim / Pod metadata / runtime registry との連携方針が明記されている。
- 実装まで含める場合は `cargo test -p yoi-workspace-server`、`cargo check -p yoi`、`cd web/workspace && deno task check && deno task build`、`git diff --check`、`nix build .#yoi --no-link` が通る。
