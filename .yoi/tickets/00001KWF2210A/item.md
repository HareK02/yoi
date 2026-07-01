---
title: 'Workspace Backend設定ファイルのスキーマを定義する'
state: 'planning'
created_at: '2026-07-01T14:41:48Z'
updated_at: '2026-07-01T14:43:04Z'
assignee: null
---

## 背景

Workspace Backend は現在、`ServerConfig::local_dev(...)` と起動時の CLI 引数で listen address、frontend URL、DB path、embedded Runtime store root などを組み立てている。これだと設定項目が増えたときに CLI flag が管理漏れしやすく、config と data の境界も曖昧になりやすい。

Backend 設定は file schema として定義し、CLI flag 追加で増やさない。CLI は起動対象 workspace を決める最小入口に留め、Backend の運用設定は固定 path の config file から読む。

この Ticket では、Workspace Backend 設定ファイルの schema、読み込み位置、default、config/data 境界を定義する。

## 目的

- Workspace Backend の設定を TOML file schema として定義する。
- 新しい Backend 設定項目を CLI flag として増やさない。
- config file に入れるものと、Backend / Runtime が生成する data を明確に分ける。
- secrets や runtime-generated state を config file に混ぜない。
- 既存の `ServerConfig` を、file config から解決された runtime config として扱えるようにする。

## 方針

### CLI flag は増やさない

この Ticket では `--config` や個別設定用 CLI flag を追加しない。Backend config file は固定 path から読む。

既存の開発用 CLI 引数を直ちに削除するかは実装時に判断するが、新規設定項目は CLI flag として追加しない。最終的には `defaults -> config file -> process environment for secrets only` 程度に寄せ、CLI override を通常運用経路にしない。

### config file path

workspace-local な local config として、固定 path を使う。

```text
<workspace_root>/.yoi/workspace-backend.local.toml
```

理由:

- workspace ごとに Backend 設定が分かれる。
- `.local` を含むため current `.gitignore` の `*.local*` で git 管理外になる。
- `.yoi/tickets` や `.yoi/objectives` など project record とは別物として扱える。
- `--config` なしで discovery できる。

config file が無い場合は defaults で起動できること。

### data root の扱い

config file は data の保存先 override を持ってよいが、data 本体は config file に入れない。

既定の data root は user data 配下を使い、workspace_id で分離する。

```text
<data_dir>/workspace-server/<workspace_id>/
```

この配下に少なくとも次を置く。

```text
workspace.db
embedded-runtime/
logs/   # 必要なら。現状の detached dev log は別扱いでもよい。
```

workspace-local `.yoi/workspace-backend.local.toml` に data path override を書けるが、DB や runtime store の中身は config ではない。

## schema 案

実装時には serde 用の `WorkspaceBackendConfigFile` 相当の型を定義する。

```toml
[server]
listen = "127.0.0.1:8787"
frontend_url = "http://127.0.0.1:5173"
static_assets_dir = null

[data]
root = null
workspace_database_path = null
embedded_runtime_store_root = null

[limits]
max_records = 200

[[runtimes.remote]]
id = "example"
endpoint = "http://127.0.0.1:8790"
token_ref = null
```

### `[server]`

Config:

- `listen`: Backend HTTP/WebSocket listen address。
- `frontend_url`: Browser-facing frontend URL を生成・表示するための URL。
- `static_assets_dir`: packaged frontend ではなく local assets を serve する場合の override。

Data ではないもの:

- active connection。
- WebSocket cursor。
- generated Runtime event。

### `[data]`

Config:

- `root`: Workspace Backend data root の override。
- `workspace_database_path`: control plane DB path の override。
- `embedded_runtime_store_root`: embedded Runtime fs-store root の override。

Data:

- SQLite DB 本体。
- Runtime fs-store の snapshot / transcript / event / ConfigBundle store。
- generated logs / pid files。

Data 本体を TOML に書かない。

### `[limits]`

Config:

- `max_records`: API response で返す project record 数の上限。

最初は `max_records` だけに留める。body truncation や git log limit まで一気に config 化しない。

### `[[runtimes.remote]]`

Config:

- `id`: remote Runtime source id。
- `endpoint`: remote Runtime endpoint。
- `token_ref`: secret store / env / local secret への参照。

禁止:

- bearer token 値を直接 TOML に保存しない。
- Runtime endpoint の credential / socket path を Browser-facing API に出さない。

## config に入れないもの

- `workspace_id` / `created_at` / workspace identity metadata。
- Ticket / Objective body。
- WorkerId / TranscriptId。
- Runtime event cursor。
- transcript entries。
- ConfigBundle 本体。
- SQLite DB の中身。
- embedded Runtime fs-store の中身。
- secret 値 / bearer token 値。
- socket path / session path / raw execution handle。
- live execution state。
- git HEAD / dirty state / log projection。

## data / record の authority

- Workspace identity は `.yoi/workspace.toml` の record として扱い、Backend config file に移さない。
- Ticket / Objective は `.yoi/tickets` / `.yoi/objectives` など project record を authority とする。
- Backend control plane DB は generated data とする。
- embedded Runtime fs-store は generated Runtime data とする。
- config file はこれらの保存先や接続先を指定するだけで、record 本体を持たない。

## 実装要件

- `WorkspaceBackendConfigFile` 相当の serde TOML schema を追加する。
- config file が無い場合は defaults で起動する。
- unknown field は拒否するか warning にするかを明示し、silent ignore にしない。
- path は workspace root からの relative path と absolute path の扱いを明確にする。
  - `data.root` relative path は workspace root 基準にする。
  - user data default は absolute path として resolve する。
- `ServerConfig` は file config と workspace identity から解決された runtime config として扱う。
- new Backend setting を CLI flag として追加しない。
- secrets は `token_ref` など ref として持ち、値を config file に保存しない。

## Non-goals

- full secret store UI の実装。
- remote Runtime supervisor の完成。
- frontend Vite dev server の config schema 化。
- Ticket / Objective schema の変更。
- Runtime fs-store の内部 schema 変更。
- 既存 CLI 引数の全面削除。ただし新規設定項目を CLI flag として増やさない。

## 受け入れ条件

- Workspace Backend config file の schema が code/docs/tests 上で明確になっている。
- fixed path `<workspace_root>/.yoi/workspace-backend.local.toml` から config を読む。
- config file が無い場合は defaults で起動できる。
- `ServerConfig` が config file + workspace identity + defaults から解決される。
- 新規 Backend 設定項目用の CLI flag が追加されていない。
- config と data の分類が code/docs/tests 上で明確になっている。
- secret 値を config file に保存しない。
- unknown field の扱いが test されている。
- relative path / absolute path の resolve が test されている。
- Backend data root / DB path / embedded Runtime store root の override が test されている。
- Browser-facing API に config file path、data root、secret、socket path、runtime store path が漏れない。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
