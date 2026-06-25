---
title: '組み込み/ネットワーク対応Worker Runtime crateを作る'
state: 'planning'
created_at: '2026-06-25T12:17:05Z'
updated_at: '2026-06-25T13:43:31Z'
assignee: null
---

## 背景

Yoi は現在、Worker 的な実行単位を主に `yoi pod` process / Unix socket / pod metadata / session jsonl として扱っている。一方で今後は、Companion や routing-only Orchestrator を Backend process 内に組み込んだ Runtime 上の Worker として動かし、Coder / Reviewer などは local / remote host 上の Runtime process で動かしたい。

このため、Backend が持つ `RuntimeRegistry` や Workspace API の都合ではなく、**Worker を動かす環境そのものとしての Runtime** を独立した crate / library API として定義する必要がある。`RuntimeRegistry` は Backend が embedded Runtime と remote Runtime をまとめて同じようにアクセスするための集約境界であり、Runtime 自体の実装主体ではない。

この Ticket では、まず要件だけを整理する。実装前に、現在の `pod`、`llm-worker`、Panel / Workspace backend における Pod / Worker 扱いを調査し、既存構造から Runtime crate へ移すべき責務と adapter として残すべき責務を把握する。

## 要件

### Runtime crate の位置付け

- `crates/worker-runtime` を Worker を動かす実行環境の正体として設計する。
- `worker-runtime/lib.rs` は Backend などに組み込める embeddable Runtime API を公開する。
- `worker-runtime/main.rs` は同じ Runtime を network API で公開する Runtime process を起動する。
- Runtime process と embedded Runtime は Worker semantics を二重実装しない。
  - Worker lifecycle / input / output / event / transcript projection は lib 側を正とする。
  - main binary は config / API server / transport / shutdown wrapper に留める。

### Runtime / Worker model

- Runtime は Worker を動かす環境であり、trait object ではなく domain entity / concrete runtime implementation として扱う。
- Runtime は複数 Worker を保持・起動・停止・操作できる。
- Worker identity は Runtime scoped とする。
  - `runtime_id`
  - `worker_id`
  - UI 用 `display_name` / `display_ref`
- Browser / API / Backend は `runtime_id + worker_id` を authority とし、`pod_name` / socket path / session path を authority にしない。
- Runtime は capability / status / diagnostics を返す。
  - backend internal
  - local process capable
  - future remote host capable
  - filesystem / shell / git / worktree capability
  - tools availability

### Embeddable Runtime API

- Backend は `Runtime` を process 内に直接組み込める。
- Embedded Runtime は tools なし Companion のような Worker を local process / socket なしで動かせる。
- API は少なくとも以下の操作を表現できる。
  - runtime summary / status
  - worker list / detail
  - create worker
  - send input
  - stop / cancel worker
  - bounded transcript projection
  - event subscription または event cursor
  - usage / overview projection
- v0 は single-flight / busy reject でよい。
- raw provider trace / raw session full log を Backend durable authority にしない。

### Networked Runtime process API

- `worker-runtime/main.rs` は Runtime を network API として公開する。
- 将来、別ホストの Runtime process に Backend が接続し、remote Worker を local / internal Worker と同じ概念で扱えるようにする。
- Network API は command と observation を分ける。
  - command: create / input / stop / cancel
  - observation: worker status / transcript projection / events
- v0 transport は実装時に選ぶが、HTTP command + SSE/WebSocket event stream を想定可能な shape にする。
- Browser が remote Runtime へ直接 authority-bearing request を送るのではなく、Backend registry / policy 経由にする。

### Backend RuntimeRegistry との関係

- Workspace backend の `RuntimeRegistry` は、embedded Runtime と remote Runtime client をまとめる集約境界とする。
- Registry は Worker を実行しない。
- Registry は runtime lookup / policy / visibility / API projection / current workspace filtering を担う。
- Backend internal Companion は embedded Runtime に載る Worker として扱う。
- Existing local Pod / process-based Worker は Runtime の adapter または移行対象として扱う。

### Existing Worker / LLM engine migration boundary

- `llm-worker` は先に `llm-engine` へ改名し、LLM turn engine と実行単位としての Worker を名前上分離する。
- 既存 `pod` crate は先に `worker` crate へ rename し、single Worker host として扱う。
- 既存 `yoi pod` process は当面 compatibility / process-backed Worker adapter として扱えるようにする。
- 最終的には process boundary を Worker ではなく Runtime に寄せる。
  - Runtime process が複数 Worker を保持できる。
  - Worker ごとに subprocess を持つかどうかは Runtime implementation detail とする。
- `llm-engine` の provider streaming / tool execution / history integration のうち、Runtime crate に移すべきものと Worker-specific に残すものを調査して整理する。
- `worker` crate の Unix socket protocol / metadata / session persistence / TUI attach semantics は、Runtime API との対応を調査してから移行方針を決める。

### Pod-named store / registry migration boundary

- `pod-store` / `pod-registry` は standalone crate として rename せず、`worker-runtime` 作成時に Runtime 内部 module へ直接統合する。
- `pod-store` 相当は Runtime 内部の Worker metadata / transcript projection / config snapshot persistence module とする。
- `pod-registry` 相当は Backend RuntimeRegistry ではなく、Runtime 内部の live Worker allocation / scope conflict / stale reclaim module とする。
- Backend `RuntimeRegistry` は embedded / remote Runtime を束ねる集約境界であり、Worker store / live allocation authority を直接持たない。
- 後方互換 alias / standalone `worker-store` / standalone `worker-registry` / old crate compatibility / old on-disk migration は設けない。

### Safety / authority

- Runtime API は raw socket path / session path / local metadata path を公開 API authority にしない。
- Worker tool authority は Runtime / Worker config / explicit grant で決める。
- Backend internal Companion v0 は filesystem / shell / git / Ticket mutation tools を持たない。
- Remote Runtime との接続では将来 auth / permission / network safety を挟める境界を残す。

## 調査事項

実装前に以下を調査する。

- `crates/pod` が現在担っている責務。
  - process entrypoint
  - socket protocol
  - session persistence
  - metadata / runtime dir
  - in-flight snapshot / attach behavior
  - tool registry / workflow / profile integration
- `crates/llm-worker` が現在担っている責務。
  - provider request / streaming
  - history / callback / tool-call loop
  - reasoning / usage / continuation / retry
  - Worker として再利用できる boundary
- Panel / TUI / Workspace backend が現在 Pod をどう扱っているか。
  - Companion send path
  - Worker/host list API
  - Pod metadata reader
  - spawn / stop / attach / event handling
- Existing `client` crate の process spawn / PodClient / socket protocol を Runtime adapter でどう扱うか。

## Non-goals

- Backend internal Companion Web Console の完成。
- Remote Runtime protocol の完成。
- Existing Pod process model の即時削除。
- Full session storage migration。
- Full tool authority redesign。
- Multi-user auth / permission model の完成。

## 受け入れ条件

この Ticket は要件整理と現状把握から開始する。ready に進める前に以下を満たす。

- `worker-runtime/lib.rs` と `worker-runtime/main.rs` の責務境界が整理されている。
- Runtime / Worker / RuntimeRegistry / Backend service の責務境界が整理されている。
- Runtime は Worker を動かす環境、Registry は Backend の集約境界であることが明確になっている。
- Embedded Runtime と networked Runtime process が同じ Runtime API に基づく方針が整理されている。
- Existing `worker` / `llm-engine` / Panel / Workspace backend の Worker 扱いの調査結果が記録されている。
- `pod-store` / `pod-registry` は standalone rename を挟まず、`worker-runtime` 内部 module へ直接統合する方針が整理されている。
- Backend `RuntimeRegistry` と Runtime internal store / allocation registry の責務境界が整理されている。
- 既存 Worker process model から Runtime model へ移行するための初期 implementation split が提案されている。
