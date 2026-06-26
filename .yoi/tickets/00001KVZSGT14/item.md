---
title: 'Backend RuntimeRegistryにremote worker-runtime processを接続する'
state: 'inprogress'
created_at: '2026-06-25T16:23:58Z'
updated_at: '2026-06-26T06:21:31Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T20:34:35Z'
---

## 背景

Standalone `worker-runtime` process が FS store、REST command server、event stream server を持った後、Workspace Backend は remote Runtime process に client として接続できる必要がある。Browser は remote Runtime に直接接続せず、Backend が RuntimeRegistry / policy / visibility / audit / typed error mapping を挟んで proxy / projection する。

この Ticket では Backend RuntimeRegistry に remote Runtime client handle を追加する。Embedded Runtime 接続とは別実装粒度とする。

## 要件

### Remote Runtime client

- Backend は config-like data から remote Runtime client handle を作成できる。
  - runtime id / display name。
  - base URL。
  - token / secret ref placeholder。
  - capability cache / last seen status。
- Remote command は `worker-runtime` REST command API に対する Backend-owned client で実行する。
- Remote observation は Runtime event stream API に対する Backend-owned client で購読または proxy できる。
- Backend は Runtime endpoint / credential を Browser に渡さない。

### Registry routing

- Backend RuntimeRegistry は remote Runtime handle へ以下を route できる。
  - runtime summary / status / capabilities。
  - worker list / detail。
  - create worker。
  - send input。
  - stop / cancel。
  - bounded transcript projection。
  - event stream/proxy。
- Embedded Runtime handle と remote Runtime handle は Browser-facing API から同じ `runtime_id + worker_id` authority で扱える。
- Network failure / auth failure / timeout / remote unsupported / remote worker not found を typed error に map する。

### Config / policy boundary

- Backend がどの remote Runtime に接続してよいかを config / registry data で管理する。
- Dynamic registration は不要。
- Config bundle sync は関連するが、この Ticket では remote Runtime connection / routing を主目的とする。
- Browser は remote Runtime base URL / token / direct endpoint authority を知らない。

## Non-goals

- `worker-runtime` core crate implementation。
- FS store implementation。
- REST command server implementation。
- Event stream server implementation。
- Embedded Runtime integration。
- Dynamic Runtime registration。
- Full auth / permission model。
- Backend internal Companion Web Console completion。

## 受け入れ条件

- Workspace Backend が remote Runtime client handle を config-like data から登録できる。
- Backend RuntimeRegistry が remote Runtime の list/detail/create/input/transcript/event operations を route できる。
- Remote network/auth/timeout errors が typed Backend errors に map される。
- Browser-facing API に remote Runtime base URL / credential / direct endpoint が露出しない。
- Embedded Runtime handle と remote Runtime handle が同じ `runtime_id + worker_id` authority model で扱われる。
- Event stream client/proxy path が Backend-owned connection として実装されている。
- Focused workspace-server tests cover mocked remote Runtime routing and error mapping.
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
