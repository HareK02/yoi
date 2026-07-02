---
title: 'Workspace初期化をinitコマンドに切り出しserveの副作用をなくす'
state: 'planning'
created_at: '2026-07-02T07:02:02Z'
updated_at: '2026-07-02T08:31:22Z'
assignee: null
---

## 背景

現状の Workspace Backend は `yoi-workspace-server serve` 起動時に workspace root を決め、その中で次の初期化を暗黙に実行している。

- `WorkspaceIdentity::load_or_init(workspace_root)`
  - `.yoi/workspace.toml` が無ければ作る。
- `WorkspaceBackendConfigFile::ensure_default_template_for_workspace(workspace_root)`
  - `.yoi/workspace-backend.default.toml` が無ければ `resources/workspace-backend.default.toml` からコピーする。

1Workspace=1Backend の現状では、Backend の実行 directory / `--workspace` が workspace root になっている。この前提自体はよいが、`serve` が初期化副作用を持つと、間違った cwd で起動しただけで `.yoi/workspace.toml` や default config template が作られる。

Workspace の一回だけ実行されるべき初期化は、明示的な `init` コマンドへ切り出し、`serve` は既に初期化済みの workspace を起動するだけにする。

## 目的

- Workspace 初期化を明示コマンドに切り出す。
- `serve` 起動時に workspace identity / default config template を作らない。
- `serve` は未初期化 workspace に対して typed diagnostic で失敗する。
- 1Workspace=1Backend の前提を保ち、workspace root は Backend 起動対象として明示する。
- 初期化で作る record / template と、serve/runtime が生成する data を分ける。
- 現状の local filesystem 保存を採用しつつ、Workspace / Project record の将来的な provider 可換性を妨げない。

## コマンド設計

### Product CLI

`yoi` 側に workspace init subcommand を追加する。

```text
yoi workspace init [--workspace <PATH>]
yoi workspace serve [OPTIONS]
```

`yoi workspace init` は `yoi-workspace-server init` を起動する thin wrapper にする。`yoi` binary は workspace-server crate を直接 link しない現在の境界を保つ。

### workspace-server CLI

`yoi-workspace-server` 側にも init command を追加する。

```text
yoi-workspace-server init [--workspace <PATH>]
yoi-workspace-server serve [OPTIONS]
```

`--workspace` は既存 `serve` と同様に、未指定なら current directory を workspace root とする。workspace root は canonicalize する。

## 初期化で作るもの

`init` は次を作る。

```text
.yoi/workspace.toml
.yoi/workspace-backend.default.toml
```

### `.yoi/workspace.toml`

- Workspace identity record。
- `workspace_id`, `created_at`, `display_name` を持つ。
- 既存ファイルがある場合は parse/validate し、上書きしない。
- 作成は `create_new` semantics を維持し、race 時は既存 record を読み直す。

### `.yoi/workspace-backend.default.toml`

- `resources/workspace-backend.default.toml` からコピーする workspace-local template。
- 既存ファイルがある場合は上書きしない。
- `.local` config ではないため、ユーザーが実設定として使う場合は `.yoi/workspace-backend.local.toml` にコピーして編集する。

## 初期化で作らないもの

`init` は Backend / Runtime data を作らない。

- `.yoi/workspace-backend.local.toml`
- control-plane SQLite DB
- embedded Runtime fs-store
- logs / pid files
- Worker / transcript / ConfigBundle data
- Ticket / Objective body

これらは config ではなく data / project record / runtime state なので、必要になった時点で各 subsystem が作る。

## Workspace storage / 正本境界

現状の Workspace 初期化は local filesystem marker を作る実装でよい。ただし、その filesystem layout を Workspace の public API contract として固定しない。

### 現状の local filesystem record

`init` が作る `.yoi/workspace.toml` は local workspace identity marker として扱う。

- 1Workspace=1Backend の現状では、Backend の実行 directory / `--workspace` が workspace root。
- `.yoi/workspace.toml` はその directory を Yoi workspace として識別する local record。
- `workspace_id` は Backend data root の key として使ってよい。
- `.yoi/workspace.toml` の raw path を Browser-facing API / Runtime create API / Worker conversation context の正本識別子として出さない。

### Backend / Runtime data

Backend DB と embedded Runtime fs-store は generated local data であり、Workspace / Project record の正本ではない。

- control-plane SQLite DB は Backend projection / control-plane data。
- embedded Runtime fs-store は Runtime snapshot / Worker snapshot / transcript / ConfigBundle store。
- これらの data path は config で override できても、data 本体は config や workspace identity ではない。
- Browser-facing API に data root、DB path、Runtime store path、raw execution handle を出さない。

### Project record provider 可換性

Ticket / Objective などの project record は、現状では `.yoi/tickets` / `.yoi/objectives` などの local filesystem backend を正本として扱ってよい。ただし、将来的に provider を差し替えられるよう、init / serve は以下を守る。

- `init` は Ticket / Objective body や provider-specific project record layout を生成しない。
- Backend DB を Ticket / Objective の正本として昇格しない。
- Runtime / Worker identity に `.yoi/tickets/...` などの filesystem path を持たせない。
- Browser-facing API は typed backend / record id 経由で扱い、filesystem path を正本識別子として公開しない。
- 将来 `ProjectRecordBackend` / `TicketBackend` / `ObjectiveBackend` 相当の provider を差し替える余地を残す。

この Ticket の成果物は local filesystem init を整えることだが、正本可換性のために上位 API へ local path を漏らさないことも受け入れ条件に含める。

## `serve` の挙動変更

`serve` は初期化済み workspace だけを起動する。

変更後:

1. workspace root を決める。
2. `.yoi/workspace.toml` を load する。
3. `.yoi/workspace-backend.default.toml` は作らない。
4. `.yoi/workspace-backend.local.toml` があれば読む。無ければ defaults。
5. resolved `ServerConfig` で Backend を起動する。

`.yoi/workspace.toml` が無い場合は失敗する。

Diagnostic 例:

```text
workspace is not initialized at <path>; run `yoi workspace init --workspace <path>` first
```

`serve` は `.yoi/workspace-backend.default.toml` が無いだけでは失敗しない方針とする。これは template であり、runtime config の authority ではないため。ただし init 済み workspace で default template が欠けている場合に warning を出すかは実装時に判断する。

## 内部 API 整理

Workspace 初期化は reusable な内部関数へ切り出す。

候補:

```rust
WorkspaceInitialization::ensure(workspace_root) -> WorkspaceInitializationReport
WorkspaceInitialization::load_required(workspace_root) -> WorkspaceIdentity
```

または既存型に近くするなら:

```rust
WorkspaceIdentity::init_if_missing(...)
WorkspaceIdentity::load_required(...)
WorkspaceBackendConfigFile::ensure_default_template_for_workspace(...)
```

重要なのは、`serve` が `load_or_init` を呼ばないこと。

## 既存 CLI flag との関係

この Ticket は Workspace Backend config schema の方針を維持し、新規 Backend 設定項目を CLI flag として増やさない。

- `init` に必要なのは `--workspace` だけ。
- `serve` の既存 legacy dev flags をこの Ticket で全面削除するかは別判断にする。
- ただし `serve` の初期化副作用は必ずなくす。

## 実装要件

- `yoi workspace init [--workspace <PATH>]` を追加する。
- `yoi-workspace-server init [--workspace <PATH>]` を追加する。
- workspace-server 側の help に init を追加する。
- `WorkspaceIdentity` に load-only path を追加する。
  - 既存 `load_or_init` は init command 内部用に残してよいが、serve からは呼ばない。
- `serve` から `WorkspaceIdentity::load_or_init(...)` を外す。
- `serve` から `WorkspaceBackendConfigFile::ensure_default_template_for_workspace(...)` を外す。
- 未初期化 workspace の `serve` は typed diagnostic で失敗する。
- `init` は existing workspace identity / default template を上書きしない。
- `init` は data root / DB / embedded Runtime store を作らない。
- `init` は Ticket / Objective など provider-specific project record layout を作らない。
- `serve` / Browser-facing API / Runtime create path に workspace-local filesystem path を正本識別子として漏らさない。

## 受け入れ条件

- `yoi workspace init [--workspace <PATH>]` が使える。
- `yoi-workspace-server init [--workspace <PATH>]` が使える。
- `init` が `.yoi/workspace.toml` と `.yoi/workspace-backend.default.toml` を作る。
- `init` が既存 `.yoi/workspace.toml` / `.yoi/workspace-backend.default.toml` を上書きしない。
- `init` が `.yoi/workspace-backend.local.toml`、DB、embedded Runtime fs-store、logs を作らない。
- `init` が Ticket / Objective body や provider-specific project record layout を作らない。
- `serve` が `.yoi/workspace.toml` を新規作成しない。
- `serve` が `.yoi/workspace-backend.default.toml` を新規作成しない。
- 未初期化 workspace で `serve` すると、`workspace init` を促す diagnostic で失敗する。
- 初期化済み workspace では config file が無くても defaults で `serve` が起動できる。
- Browser-facing API / Runtime create path / Worker conversation context に `.yoi/workspace.toml`、`.yoi/tickets/...`、data root、DB path、Runtime store path が正本識別子として漏れない。
- Project record provider 可換性を妨げないことが code/docs/tests 上で明確になっている。
- Help text が `workspace init` と `workspace serve` の責務差を説明している。
- Focused tests が init 作成、init idempotency、serve 未初期化拒否、serve 初期化済み起動、data 非作成を確認する。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo test -p yoi` が通る、または CLI parser tests が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。

## 対象外

- Backend config schema の追加変更。
- frontend Vite config の管理。
- remote Runtime supervisor。
- secret store 実装。
- existing legacy `serve` flags の全面削除。
- Ticket / Objective init の統合。
