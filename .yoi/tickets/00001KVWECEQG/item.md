---
title: 'Abstract Worker runtime registry and overview reporting'
state: 'queued'
created_at: '2026-06-24T09:11:38Z'
updated_at: '2026-06-24T09:22:55Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-24T09:22:55Z'
---

## 背景

Workspace backend には現状 `LocalRuntimeBridge` があり、local Pod metadata から `HostSummary` / `WorkerSummary` を返す read-only live view として動いている。しかしこれは concrete local 実装に近く、将来の Backend internal runtime、local Pod runtime、remote / multi-machine runtime を同じ control plane から扱う抽象境界にはまだなっていない。

今後の Ticket / Worker 管理 UI では、Backend が Worker を直接 `yoi pod` や local metadata path として扱うのではなく、複数の Worker runtime を束ねる control plane になる必要がある。Companion / Intake / routing-only Orchestrator のような filesystem を必須としない Worker は Backend internal runtime で動かせるべきであり、Coder / Reviewer / build-heavy Worker は local / remote の filesystem-capable runtime に配置できるべきである。

また、Worker session を Backend が raw log として全量・恒久的に収集する設計にはしない。raw session / provider trace / detailed event stream は execution runtime 側の prune 可能な debug/source log とし、Backend は Worker run overview、usage aggregate、lifecycle event、Ticket / role / runtime link、artifact reference、failure summary のような後から使える durable projection を持つ。

この Ticket では、既存 `LocalRuntimeBridge` をレベル上げして、Workspace backend の Worker runtime registry / runtime abstraction として整理する。

## 目的

- Workspace backend が複数の Worker runtime を扱える抽象境界を持つ。
- 既存 local Pod metadata read-only bridge を `LocalPodRuntime` 相当の runtime 実装として位置付ける。
- Backend が runtime registry を持ち、API handler が concrete local bridge に直接依存しないようにする。
- raw session を Backend authority にせず、overview / metrics / durable outcome を中心に扱う方針を型と API 境界に反映する。
- 後続の spawn / stop / protocol proxy / remote runtime / internal runtime 実装に進める基盤を作る。

## 要件

### Worker runtime abstraction

- `LocalRuntimeBridge` をそのまま API handler から直接使う構造をやめ、Worker runtime trait / service 境界を導入する。
- 概念名は実装時に決めてよいが、`WorkerRuntime` / `WorkspaceWorkerRuntime` / `RuntimeRegistry` / `WorkerRuntimeRegistry` のような責務が明確な名前にする。
- v0 では少なくとも以下の read operations を抽象化する。
  - runtime / host list
  - runtime / host detail
  - worker list
  - worker detail / lookup
- 後続 operation の接続点を型として残す。
  - spawn worker
  - stop worker
  - send input / interrupt / compact などの worker command routing
  - bounded transcript / debug session read
  - event stream / run overview stream
- v0 で未実装の operation は unsupported として typed error を返せるようにする。

### Runtime registry

- Workspace backend は単一の concrete `LocalRuntimeBridge` ではなく、runtime registry 経由で Worker / Host を取得する。
- registry は複数 runtime を保持できる構造にする。
  - backend internal runtime
  - local Pod runtime
  - future remote runtime
- v0 では local Pod runtime のみ登録してよい。
- runtime_id / host_id / worker_id は API contract 上 opaque id として扱う。
- `pod_name` は local Pod runtime の implementation detail / display hint とし、外部操作の主キーにはしない。
- API handler は runtime_id / worker_id を registry に解決させ、raw local path / socket path / metadata path を browser に出さない。

### Runtime capability model

- Runtime / Host detail は capability summary を返す。
- 少なくとも以下のような区別ができること。
  - can_list_workers
  - can_get_worker
  - can_spawn_worker
  - can_stop_worker
  - can_accept_input
  - can_stream_events
  - can_read_bounded_transcript / debug session
  - has_workspace_fs
  - has_shell
  - has_git
  - supports_worktrees
  - supports_backend_internal_tools / ticket tools
- Companion / Intake / routing-only Orchestrator は filesystem なし runtime でも動かせる、Coder / Reviewer は filesystem-capable runtime が必要、という placement 判断を後続で表現できる shape にする。

### Worker identity / visibility

- Worker identity は runtime scoped に扱う。
  - runtime_id
  - worker_id
  - optional active run id
  - optional active session id / segment id は implementation detail または debug field
- Workspace UI の通常表示では current workspace から見える Worker だけを返す。
- local Pod runtime では `PodMetadata.workspace_root` を current workspace root と比較して visibility を決める。
- `cwd` と runtime workspace identity を混同しない。
  - dedicated Orchestrator worktree があっても、runtime workspace identity は original/main workspace root である。
- workspace root が無い / 不一致 / metadata invalid な Worker は diagnostics または debug scope として扱い、通常 UI の authority を歪めない。

### Session / overview boundary

- Backend は raw session log を恒久 authority として持たない。
- raw session / provider trace / verbose event stream は execution runtime 側に残し、runtime retention / prune 対象とする。
- Backend が durable に持つものは以下を中心にする。
  - Worker run overview
  - lifecycle event
  - usage aggregate
  - Ticket / role / runtime / worker link
  - durable outcome summary
  - failure summary
  - artifact / report reference
- Runtime interface は必要に応じて bounded transcript / debug session tail を読める接続点を持つが、これは debug / live UI 用であり Backend DB の durable authority ではない。
- usage dashboard は raw session ingest ではなく、runtime からの usage event / aggregate を元に構築できる形にする。

### Backend internal runtime planning

- Backend internal runtime はこの Ticket で完全実装しなくてよいが、registry / capability / identity model がそれを表現できること。
- Backend internal runtime は filesystem tool を前提にしない Worker 用とする。
  - Companion
  - Intake
  - Ticket planning / routing assistant
  - Dashboard assistant
- Backend internal Worker は Ticket / Workspace API を tool として使う形を想定し、raw local filesystem authority を要求しない。

### Local Pod runtime migration

- 既存 `LocalRuntimeBridge` の read-only local metadata 実装を `LocalPodRuntime` 相当へ移す、または薄い adapter として registry に登録する。
- 既存 API の挙動を壊さない。
  - `GET /api/hosts`
  - `GET /api/workers`
  - `GET /api/hosts/{host_id}/workers`
- 追加可能であれば worker detail endpoint を追加する。
  - `GET /api/workers/{worker_id}`
  - `GET /api/hosts/{host_id}/workers/{worker_id}`
- local Pod metadata の raw content / snapshot content / socket path は API response に出さない。

### API / error boundary

- API response は runtime registry 経由の typed response にする。
- unknown runtime / host / worker は typed error として返す。
- runtime unavailable、operation unsupported、worker not visible、invalid query は区別できるようにする。
- Browser は local Unix socket path、runtime registry file path、Pod metadata path、raw session path を authority として渡さない。

## Non-goals

- Worker spawn / stop の本実装。
- Remote runtime protocol の本実装。
- Backend internal runtime で実際に LLM Worker を起動すること。
- raw session 全量の Backend DB ingest。
- Session log schema migration。
- Ticket storage migration。
- Worker operation UI の完成。
- SSE/WebSocket event stream の完成。
- Permission / auth model の完成。

## 受け入れ条件

この Ticket は planning から開始する。ready に進める前に以下を満たす。

- Worker runtime / runtime registry / local Pod runtime の責務境界が明文化されている。
- 既存 `LocalRuntimeBridge` をどう level-up / rename / adapter 化するかが決まっている。
- Backend が複数 runtime を保持できる registry 構造の方針が決まっている。
- runtime_id / host_id / worker_id の identity 方針が決まっている。
- local Pod `pod_name` を external operation key にしない方針が明記されている。
- Runtime capability model の最小 field が整理されている。
- current workspace visibility と workspace_root / cwd の扱いが明記されている。
- raw session を Backend durable authority にしない方針が明記されている。
- Backend に残す Worker run overview / usage aggregate / lifecycle projection の方針が明記されている。
- bounded transcript / debug session read は runtime-local optional operation とする方針が明記されている。
- 既存 `/api/hosts` / `/api/workers` を registry 経由へ移す方針が明記されている。
- 実装まで含める場合は `cargo test -p yoi-workspace-server`、`cargo check -p yoi`、`git diff --check`、`nix build .#yoi --no-link` が通る。
