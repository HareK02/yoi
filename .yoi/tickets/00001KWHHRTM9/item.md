---
title: 'Workspace Backend Runtime接続の管理画面と永続configを追加する'
state: 'ready'
created_at: '2026-07-02T13:54:52Z'
updated_at: '2026-07-02T16:25:43Z'
assignee: null
---

## 背景

Workspace Backend は embedded Runtime を process 内に持ち、remote Runtime は `ServerConfig.remote_runtime_sources` から `RuntimeRegistry` に登録する構造になっている。Runtime が Backend へ接続しに来るのではなく、Backend が Runtime source を registry に登録して呼び出す。

現状、remote Runtime 接続の永続化は `.yoi/workspace-backend.local.toml` の `[[runtimes.remote]]` schema として存在する。ただし、これを Browser から追加・削除・確認する管理画面/API はまだ無い。また `token_ref` は field としてあるが secret ref resolution は未実装で、raw token 値を config に保存する schema は無い。

手動 Coding Worker 作成フォームで Runtime を選ぶには、先に Runtime connection を Workspace Backend の設定として管理できる必要がある。この Ticket では Runtime 接続管理 UI/API と config 永続化を追加する。

## 目的

- Workspace Browser から Runtime connection を確認・追加・削除できるようにする。
- Runtime connection config を `.yoi/workspace-backend.local.toml` に永続化する。
- raw token 値、Runtime internal store path、socket path、event cursor などを保存・表示しない。
- RuntimeRegistry の live state、persisted config、connection check / negotiation result の境界を明確にする。
- 手動 Worker 作成 UI が選択できる Runtime 候補の基盤を作る。

## 依存関係

- `00001KWHJ0XH6 Workspace BrowserにSettings/Admin画面のshellとnavigationを追加する`
  - Settings/Admin entry point / route / shell / section navigation は先行 Ticket で用意する。
  - この Ticket はその Settings shell 上に Runtime Connections section と Backend API / config persistence を追加する。

## 現状整理

### embedded Runtime

- Backend process 内で作られる built-in Runtime。
- `RuntimeRegistry::for_workspace(EmbeddedWorkerRuntime::...)` で登録される。
- config file で追加・削除する対象ではない。
- 管理画面では built-in として表示する。

### remote Runtime

- `.yoi/workspace-backend.local.toml` の `[[runtimes.remote]]` から読み込まれる。
- `ServerConfig.remote_runtime_sources` を経由して `RuntimeRegistry` に登録される。
- 現状は起動時登録だけで、UI/API からの追加・削除は無い。
- `token_ref` は schema にあるが secret resolution は未実装なので、v0 では fail closed か未対応表示にする。
- 現状は明示的な negotiation / handshake は無い。
- Backend は remote Runtime 操作時に `GET /v1/runtime`、`GET /v1/workers` などを呼び、失敗時は diagnostic を返す。
- `GET /v1/runtime` が事実上の疎通確認になっているが、protocol version、API compatibility、spawn/input/event stream support を接続時に合意する仕組みではない。
- Runtime connection management の `test` は、この現状を踏まえて endpoint reachability だけでなく lightweight negotiation / compatibility check として設計する。

## Config schema

既存 schema を使う。

```toml
[[runtimes.remote]]
id = "example"
endpoint = "http://127.0.0.1:8790"
display_name = "Example Runtime"
# token_ref = "local:example-runtime-token"
```

保存するもの:

- `id`
- `endpoint`
- `display_name`
- `token_ref`（secret ref resolution が実装されるまで v0 では UI から設定不可でもよい）

保存しないもの:

- bearer token 値。
- connection health result。
- Runtime worker list。
- Runtime event cursor。
- socket path / session path。
- Runtime fs-store path。
- live connection handle。

## Backend API 設計

Workspace Backend に Runtime connection settings API を追加する。

候補:

```http
GET    /api/settings/runtime-connections
POST   /api/settings/runtime-connections
DELETE /api/settings/runtime-connections/{id}
POST   /api/settings/runtime-connections/{id}/test
```

### GET

返すもの:

- embedded Runtime summary。
- persisted remote Runtime connection configs。
- live RuntimeRegistry projection との対応。
- sanitized diagnostics。

返さないもの:

- config file path。
- secret 値。
- Runtime endpoint credential。
- raw socket / session / store path。

### POST

request v0:

```json
{
  "id": "remote-dev",
  "endpoint": "http://127.0.0.1:8790",
  "display_name": "Remote Dev Runtime"
}
```

v0 では `token_ref` は未対応でもよい。対応する場合も raw token 値は受け取らず、secret ref だけにする。

挙動:

- id / endpoint / display_name を validate する。
- duplicate id は拒否する。
- `.yoi/workspace-backend.local.toml` を read-modify-write する。
- unknown TOML fields を壊さない方針が必要。実装が難しい場合は typed schema 全体を parse/serialize し、comments が失われることを diagnostic / docs に明記する。

### DELETE

- 指定 id の remote Runtime connection を config から削除する。
- embedded Runtime は削除不可。
- 存在しない id は typed diagnostic。

### TEST / negotiation

`test` は単なる ping ではなく、remote Runtime の lightweight negotiation / compatibility check として扱う。

v0 の check:

1. endpoint の URL / scheme を validate する。
2. Backend-owned HTTP client で `GET /v1/runtime` を呼ぶ。
3. JSON response が `worker-runtime` の runtime summary として parse できるか確認する。
4. observed runtime id / display name / status / capabilities を読む。
5. Browser が必要とする操作に対して compatibility を判定する。
   - list workers。
   - observe worker detail。
   - event websocket endpoint を構成できること。
   - spawn worker support。
   - input dispatch support。
   - ConfigBundle availability check / sync support。
6. diagnostics を sanitized して返す。

現行 Runtime API に protocol version field が無い場合、fake version を作らない。代わりに次のように表現する。

```json
{
  "protocol": {
    "compatible": true,
    "basis": "v1 runtime endpoint responded"
  }
}
```

version / feature negotiation field が必要なら、この Ticket 内で既存 API を確認して追加するか、別 Ticket に分ける。

`test` の response は config persistence と分ける。

保存しないもの:

- observed capabilities。
- observed runtime status。
- checked_at。
- health result。
- diagnostics。

v0 ではこれらは API response / UI 表示だけに使う。保存が必要になった場合は Backend DB の observation / cache として扱い、`.yoi/workspace-backend.local.toml` には入れない。

Unsupported Runtime の扱い:

- `test` では `compatible=false` と typed diagnostic を返す。
- `POST` で保存前 check を必須にするかは実装時に決める。
- 保存を許す場合でも UI は `not_compatible` / `restart will not make this usable` などの diagnostic を表示する。

## Live反映方針

v0 は config persistence を優先し、live RuntimeRegistry への hot register/unregister は必須にしない。

選択肢:

1. config 更新後に `restart_required = true` を返す。
2. 追加だけ live register し、削除は restart required にする。
3. 追加・削除とも live反映する。

この Ticket の v0 は **1** を基本とする。UI は「保存後、Backend restart で有効化」と表示する。

理由:

- RuntimeRegistry の unregister は worker selection / observation / console subscription への影響がある。
- live / negotiated runtime state と persisted config を混ぜると責務が増える。
- まずは永続 config 管理を作り、live反映は別 Ticket に分けられる。

## UI 設計

Settings / Runtime Connections 画面を追加する。

表示:

- Embedded Runtime
  - id
  - display name
  - built-in badge
  - status / diagnostics
  - delete不可
- Remote Runtime connections
  - id
  - display name
  - endpoint
  - persisted / active / restart required / compatible status
  - Test
  - Delete

追加 form:

- id
- display name
- endpoint

v0 では token / secret input は出さない。secret ref 対応を入れる場合は separate field とし、raw token 値を入力・保存しない。

## Config read/write boundary

`.yoi/workspace-backend.local.toml` は既に `WorkspaceBackendConfigFile` として parse される。Runtime connection management はこの file を read-modify-write する。

要確認:

- comments を維持するか。
- formatting を維持するか。
- unknown fields の扱い。

v0 は typed TOML serialize で comments が失われてもよいが、その場合は実装 report と docs に明記する。comments を維持したい場合は TOML document editing crate の導入を検討する。

## 実装要件

- Runtime connection settings API を追加する。
- `.yoi/workspace-backend.local.toml` の `[[runtimes.remote]]` を read-modify-write できる。
- embedded Runtime は built-in connection として表示され、削除不可。
- remote Runtime connection の add/delete/test negotiation ができる。
- v0 では config 更新後 `restart_required = true` を返す。
- raw token 値を request / response / config file に持たない。
- RuntimeRegistry live state、persisted config、negotiation result を混同しない。
- Settings / Runtime Connections UI を追加する。
- Manual Coding Worker 作成 UI で使う Runtime candidate 情報に接続できる projection を用意する。

## 受け入れ条件

- Runtime Connections 管理画面がある。
- embedded Runtime が built-in / delete不可として表示される。
- remote Runtime connection を追加でき、`.yoi/workspace-backend.local.toml` に保存される。
- remote Runtime connection を削除でき、config から消える。
- remote Runtime connection の test negotiation ができ、compatibility / capabilities / diagnostics が sanitized response として返る。
- config 更新 response が `restart_required = true` を返す。
- negotiation result / observed capabilities / health result を `.yoi/workspace-backend.local.toml` に保存しない。
- raw token 値を UI/API/config に保存・表示しない。
- Runtime endpoint credential / socket path / session path / Runtime store path が Browser-facing API に漏れない。
- duplicate id / invalid endpoint / incompatible runtime / embedded delete attempt が typed diagnostic になる。
- Focused tests が API list/add/delete/test negotiation、config persistence、sanitization、restart_required、UI render/submit path を確認する。
- `cd web/workspace && deno task test` が通る。
- `cd web/workspace && deno task check` が通る。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。

## 対象外

- RuntimeRegistry の live unregister 完成。
- Remote Runtime workspace provisioning。
- Secret store UI。
- raw bearer token の保存。
- Manual Coding Worker 作成 form の実装。
- Runtime health monitoring daemon。
- Runtime event cursor 永続化。
