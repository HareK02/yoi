---
title: 'Backend RuntimeRegistryにembedded worker-runtimeを接続する'
state: 'closed'
created_at: '2026-06-25T16:23:58Z'
updated_at: '2026-06-26T17:46:04Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T16:31:30Z'
---

## 背景

`worker-runtime` core crate と Backend `RuntimeRegistry` 基盤ができたら、Workspace Backend process 内に embedded `worker_runtime::Runtime` を組み込み、`backend-internal` Runtime として Registry から扱えるようにしたい。これは Backend internal Companion Web Console の前提であり、remote Runtime process / HTTP client / FS store / event stream server を待たずに進められる。

この Ticket では embedded Runtime handle を Backend Registry に接続する。Runtime は memory store / builtin/default Profile fallback / toolsなし Worker でよい。

## 要件

### Embedded Runtime registration

- Workspace Backend が `worker_runtime::Runtime` を process 内に生成・保持できる。
- Embedded Runtime を `backend-internal` 相当の runtime id / display name / capabilities で RuntimeRegistry に登録できる。
- Registry から embedded Runtime の runtime summary / status / capabilities を取得できる。
- Embedded Runtime は HTTP endpoint / token / socket path を持たない。

### Worker operations

- Backend Registry は embedded Runtime に対して direct lib call で以下を route できる。
  - worker list / detail。
  - create worker。
  - send input。
  - stop / cancel。
  - bounded transcript projection。
  - event cursor / subscription placeholder。
- v0 は toolsなし Worker / builtin/default Profile fallback / memory store でよい。
- Busy / unknown worker / runtime unavailable / unsupported operation を typed error に map する。

### Backend API exposure

- Browser-facing API は embedded Runtime を remote Runtime と同じ `runtime_id + worker_id` authority で扱う。
- Browser は embedded Runtime internals / store path / provider credentials を知らない。
- Existing `/api/workers` / runtime list に embedded Runtime Worker を含めるか、新 runtime-scoped endpoint に出すかを実装時に決め、方針を記録する。
- Local compatibility source と embedded Runtime source を diagnostics / implementation kind で区別できる。

## Non-goals

- Remote Runtime process client。
- FS store。
- REST command server。
- Event stream server。
- Full Companion Web Console UI。
- Profile/config bundle sync。
- Local compatibility path の削除。

## 受け入れ条件

- Workspace Backend が embedded `worker_runtime::Runtime` を生成し、RuntimeRegistry に登録できる。
- Backend API から `backend-internal` Runtime の summary/status/capabilities を確認できる。
- Backend Registry が embedded Runtime の worker list/detail/create/send input/transcript projection を direct lib call で扱える。
- v0 toolsなし Worker が create でき、input acceptance まで確認できる。
- Browser-facing API に socket path / session path / runtime internal store path / provider credential が露出しない。
- Local compatibility source と embedded Runtime source が混同されない。
- Focused workspace-server tests cover embedded runtime registration and routing.
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
