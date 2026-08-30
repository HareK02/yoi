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
`worker` の direct entrypoint、`worker-runtime` の fresh/restore factory、`standalone` は同じ controller lifecycle を使う。fresh runtime/standalone construction は `WorkerBootstrap` を共有し、Runtime は `prepare()` 後かつ Feature install/controller exposure 前に Workdir、observation、Flow の live binding を追加する。restore は replay 済み Worker を `PreparedWorker` へ渡して同じ pre-exposure lifecycle を通す。

## authority と lifecycle

- launch は `ProfileExecutionTarget::Standalone` で built-in/XDG Profile を resolve する。repository-local Profile と path selector は authority にしない。
- canonical cwd を `WorkerFilesystemAuthority::local` の root/cwd とし、host ごとに top-level Worker と process-owned `WorkdirSession` を一つ作る。
- Controller transport は `InProcess` に固定する。通常起動で Worker subprocess、HTTP/WS server、Unix socket を作らない。
- model provider は通常の resolved Manifest から構築する。埋め込み host と deterministic test は `start_with_model_client` で同じ bootstrap に process-owned client を注入できる。
- feature plan/install は既存 `WorkerController` が行う。Task や optional direct SubWorker を standalone 側で再実装しない。
- `shutdown()` は既存 `Method::Shutdown` を送り、controller が active run、SubWorker registry、Workdir session、MachineScope allocation を順に片付けた後の confirmation を待つ。
- startup error は category のみを公開し、credential、prompt 本文、session metadata、内部 path を error text に含めない。

## CLI / TUI routing

- `yoi` の connection-aware command は `TargetKind::Standalone | Backend` の二択で dispatch する。`--local` と client config の `default_connection = "local"` は Standalone を選ぶ入力であり、旧 LocalBackend を有効化しない。 Client config は repository `.yoi/client.config.toml` を読まず、repository `.yoi/workspace.toml` は Backend Workspace identity が必要な場合だけ参照する。
- Standalone の通常起動は `StandaloneHost`、restore は専用 `StandaloneStore` の session picker を使う。Workspace Worker list、PID、Unix socket、subprocess は探索しない。
- `workers`、Backend Worker restore、Workspace panel、Ticket、Objective は Backend authority を要求する。Standalone から repository-local filesystem backend へ fallback しない。
- `yoi worker` は Runtime や明示的な process-owned integration が使う direct Worker entrypoint として残るが、通常の `yoi` / TUI 起動経路からは呼び出さない。

## 非目標

- TUI/CLI routing や画面実装
- Runtime / Workspace Server / Orchestrator / Ticket authority の内包
- standalone 独自の HTTP/WS API
- Worker/Tool/Task/SubWorker protocol の fork
- subprocess Worker launcher や runtime-owned Worker catalog
