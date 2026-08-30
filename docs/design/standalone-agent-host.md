# Standalone Agent Host

`crates/standalone` は、既存の Yoi Worker を同一プロセス内で起動する最小の host 境界である。
新しい Agent 実行系ではなく、`manifest`、`worker`、`session-store`、`workdir` の既存契約を固定構成で組み立てる。

## 依存方向

```text
standalone
  ├─ manifest
  ├─ worker
  ├─ session-store
  └─ protocol

worker
  └─ WorkerBootstrap / start_worker_controller
```

`standalone` は `tui`、`worker-runtime`、`yoi-workspace-server` に依存しない。
`worker` の direct entrypoint と `standalone` は同じ `start_worker_controller` lifecycle を使い、後者は `WorkerBootstrap` で fresh Worker construction も共有する。

## authority と lifecycle

- launch は `ProfileExecutionTarget::Standalone` で built-in/XDG Profile を resolve する。repository-local Profile と path selector は authority にしない。
- canonical cwd を `WorkerFilesystemAuthority::local` の root/cwd とし、host ごとに top-level Worker と process-owned `WorkdirSession` を一つ作る。
- Controller transport は `InProcess` に固定する。通常起動で Worker subprocess、HTTP/WS server、Unix socket を作らない。
- model provider は通常の resolved Manifest から構築する。埋め込み host と deterministic test は `start_with_model_client` で同じ bootstrap に process-owned client を注入できる。
- feature plan/install は既存 `WorkerController` が行う。Task や optional direct SubWorker を standalone 側で再実装しない。
- `shutdown()` は既存 `Method::Shutdown` を送り、controller が active run、SubWorker registry、Workdir session、MachineScope allocation を順に片付けた後の confirmation を待つ。
- startup error は category のみを公開し、credential、prompt 本文、session metadata、内部 path を error text に含めない。

## 非目標

- TUI/CLI routing や画面実装
- Runtime / Workspace Server / Orchestrator / Ticket authority の内包
- standalone 独自の HTTP/WS API
- Worker/Tool/Task/SubWorker protocol の fork
- subprocess Worker launcher や runtime-owned Worker catalog
