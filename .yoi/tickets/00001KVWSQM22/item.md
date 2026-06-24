---
title: 'Backend internal Orchestrator runtime for Kanban operations'
state: 'inprogress'
created_at: '2026-06-24T12:29:58Z'
updated_at: '2026-06-24T19:15:09Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-24T19:04:55Z'
---

## 背景

Workspace / Kanban UI から Ticket を `ready -> queued` する操作は、単なる表示変更ではなく「Orchestrator が routing / start-if-unblocked を開始してよい」という human gate として扱いたい。一方で、現状の Orchestrator は local Pod として起動・接続・通知される形に寄っており、Kanban 操作を transient Pod notification に直接結びつけると、Pod lifetime / socket / session / workspace cwd / local filesystem authority に依存しやすい。

今後の Workspace backend は Worker runtime registry を持つ control plane になり、Backend internal runtime、local Pod runtime、remote / multi-machine runtime を束ねる方向に進めたい。Kanban 操作を Orchestration に接続するなら、routing / decision / spawn intent を担当する Orchestrator は、filesystem-capable Pod ではなく Backend internal runtime 上の Worker として扱う方が自然である。

ただし、Backend internal Orchestrator は実装者ではなく control-plane の判断者 / ルーターであり、Bash / raw filesystem / raw socket path / raw session 全量読みのような authority を持つべきではない。実装・review・build・worktree 操作は、local / remote の filesystem-capable runtime 上の Coder / Reviewer / helper Worker に委譲する。

この Ticket では、Kanban operation と Orchestration を durable event / Backend internal Worker / WorkerRuntime registry 経由で接続する設計を planning する。

## 目的

- Kanban の Ticket state operation を Orchestration の durable trigger として扱う設計を固める。
- Orchestrator を Backend internal runtime 上の Worker として表現する設計を固める。
- Backend internal Orchestrator が使う tool / operation surface を、Pod 用 filesystem tools ではなく domain-specific backend tools として整理する。
- routing decision、spawn intent、overview / audit projection を raw session に依存せず残せるようにする。
- Coder / Reviewer / worktree / shell 操作は filesystem-capable runtime に委譲する境界を明確にする。

## 要件

### Kanban operation / orchestration event

- Workspace / Kanban UI の Ticket operation は backend API を通じて実行する。
- `ready -> queued` は「Orchestrator routing を開始してよい」という human gate として扱う。
- Kanban operation は transient Pod notify ではなく、durable orchestration event を生成する。
  - 例: `ticket_queued`, `ticket_state_changed`, `ticket_returned_to_planning`, `ticket_done`。
- orchestration event は actor / source / ticket_id / before state / after state / timestamp / request id を持つ。
- event は再処理・重複・backend restart に耐える形にする。
- UI click から直接 local Pod socket や raw runtime path へ通知しない。

### Backend internal Orchestrator Worker

- Backend internal runtime 上に Orchestrator Worker を置ける設計にする。
- Backend internal Orchestrator は filesystem / shell / git authority を前提にしない。
- Backend internal Orchestrator は Kanban / Ticket / runtime registry event を読み、routing decision を行う。
- Orchestrator の durable output は raw session ではなく以下を中心にする。
  - routing decision
  - orchestration plan / blocker / waiting reason
  - spawn intent
  - worker run overview
  - usage aggregate / lifecycle projection
  - failure summary / escalation request
- Backend internal Orchestrator が停止・再起動しても、未処理 event / in-progress decision の扱いが壊れないようにする。

### Tool / operation surface redesign

Backend internal Orchestrator に渡す tool / operation surface は、Pod 用の汎用 filesystem tools ではなく、Backend domain operation として設計する。

最低限検討する tool / operation:

- Ticket list / show / comment / state transition。
- Ticket relation / orchestration plan read/write。
- Kanban event read / ack / defer / fail。
- Runtime registry list / capability query / worker lookup。
- Worker spawn intent create / dispatch。
- Worker run overview read / append。
- Usage aggregate read。
- Audit / decision record append。

原則として渡さないもの:

- `Bash`。
- raw `Read` / `Write` / `Edit` over workspace filesystem。
- raw Unix socket path 操作。
- raw session 全量 ingest / unrestricted read。
- local Pod metadata path / session path を直接 authority とする操作。

### WorkerRuntime registry connection

- Backend internal Orchestrator は、実装作業を直接行わず、WorkerRuntime registry に typed spawn intent を渡す。
- runtime selection は capability を見る。
  - routing-only / intake / dashboard assistant は Backend internal runtime でもよい。
  - coder / reviewer / worktree / git / shell / build が必要な作業は filesystem-capable local / remote runtime を必要とする。
- spawn intent は raw process launch config ではなく role / ticket_id / required capabilities / workspace identity / cwd semantics / profile/workflow selection intent を表す。
- local Pod runtime はその intent を低レベル Pod process launch config へ解決する adapter として扱う。

### Worker identity / API / DB shape

- UI 表示では human-readable な `worker-name@runtime-name` 形式を使う。
  - 例: `super-duper-pod@runtimeA`。
  - これは表示用 label / `display_ref` であり、操作対象の authority にはしない。
- API の canonical identity は `runtime_id` + `worker_id` とする。
  - `runtime_id` / `worker_id` は opaque id として扱う。
  - `pod_name` は local Pod runtime の implementation detail / display hint とする。
  - Browser から `display_ref`、`pod_name`、runtime display name を authority として渡さない。
- Worker list/detail response は少なくとも以下を返せる形にする。
  - `runtime_id`
  - `worker_id`
  - `display_name`
  - `runtime_display_name`
  - `display_ref`
  - `implementation.kind`
  - local Pod runtime の場合のみ diagnostic/display 用 `implementation.pod_name`
- API path は runtime scoped identity を表現する。
  - 例: `GET /api/runtimes/{runtime_id}/workers/{worker_id}`。
  - 横断 list は `GET /api/workers` とし、各 item に `runtime_id` / `worker_id` / `display_ref` を含める。
- DB では `UNIQUE(runtime_id, worker_id)` を必須にする。
- DB の primary key は composite PK でもよいが、run overview / lifecycle event / usage aggregate からの参照を考えると、surrogate `id` + `UNIQUE(runtime_id, worker_id)` を優先候補とする。
- Worker run / lifecycle / usage / overview record は surrogate worker record id を参照できる形にしつつ、external API では runtime scoped identity を正とする。

### Session / overview boundary

- Backend は raw session log を durable authority として全量保持しない。
- Backend internal Orchestrator の decision / overview / audit を durable projection とする。
- raw session / verbose event stream / provider trace は runtime-local debug/source log とし、必要時に bounded read できる程度に留める。
- Kanban / Orchestration UI は raw transcript ではなく overview / decision / worker state / usage aggregate を表示する。

### Safety / authority

- Kanban UI click から Backend internal Orchestrator が直接 shell/git/filesystem を実行する設計にしない。
- implementation side effect 前には、既存 Ticket lifecycle authority に従って `queued -> inprogress` acceptance と decision record を残す。
- dependency / conflict / dirty workspace / missing requirement / runtime unavailable の場合は、spawn せず decision / blocker / waiting reason を記録する。
- Backend internal Orchestrator の tool set は permission/auth model を後から挟める境界にする。
- Browser は local path / socket / runtime registry path / raw session path を authority として渡さない。

## Non-goals

- Backend internal Orchestrator の full implementation。
- Kanban UI の完成。
- Coder / Reviewer spawn の本実装。
- Remote runtime protocol の本実装。
- raw session 全量の Backend DB ingest。
- Ticket storage の DB migration。
- Permission / auth model の完成。
- Worktree / git 操作を Backend process に直接持たせること。

## 受け入れ条件

この Ticket は planning から開始する。ready に進める前に以下を満たす。

- Kanban operation から durable orchestration event を生成する方針が明文化されている。
- `ready -> queued` を Orchestrator routing human gate として扱う方針が明記されている。
- Backend internal Orchestrator Worker の責務と non-responsibility が明確になっている。
- Backend internal Orchestrator に渡す domain-specific tool / operation surface が整理されている。
- Bash / raw filesystem / raw socket / raw session 全量 ingest を internal Orchestrator authority にしない方針が明記されている。
- WorkerRuntime registry と spawn intent の接続方針が明記されている。
- UI 表示は `worker-name@runtime-name` 形式を使い、API authority にはしない方針が明記されている。
- API canonical identity は `runtime_id` + `worker_id` とし、runtime scoped opaque id として扱う方針が明記されている。
- DB は surrogate worker record id + `UNIQUE(runtime_id, worker_id)` を優先候補とし、run overview / lifecycle / usage 参照に使える方針が明記されている。
- filesystem-capable work は local / remote runtime 上の Coder / Reviewer / helper Worker に委譲する方針が明記されている。
- raw session ではなく overview / decision / lifecycle / usage aggregate を Backend durable projection とする方針が明記されている。
- failure / blocker / retry / event ack semantics の初期方針が整理されている。
- 実装まで含める場合は `cargo test -p yoi-workspace-server`、`cargo check -p yoi`、`git diff --check`、必要に応じて `nix build .#yoi --no-link` が通る。
