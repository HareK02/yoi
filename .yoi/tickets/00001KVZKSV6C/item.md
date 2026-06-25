---
title: 'Backend RuntimeRegistryの基盤をworker-runtime向けに整理する'
state: 'inprogress'
created_at: '2026-06-25T14:44:03Z'
updated_at: '2026-06-25T16:58:32Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-25T16:31:28Z'
---

## 背景

Workspace Backend は複数 Runtime を束ねる `RuntimeRegistry` を持つ。Registry は Worker を実行する主体ではなく、embedded Runtime と remote Runtime process、既存 local Worker compatibility adapter を同じ Backend-facing API から参照・routing するための集約境界である。

この Ticket は embedded Runtime 実装や remote HTTP client 実装を含めない。先に Backend 側の Registry 構造、runtime identity、capability/status projection、Browser-facing API の authority 境界を `worker-runtime` の domain model に合わせて整理する。

## 要件

### Registry responsibility

- Backend `RuntimeRegistry` は Runtime を実行しない。
- Backend `RuntimeRegistry` は以下を扱う集約境界とする。
  - Runtime lookup。
  - Runtime summary / capability / status projection。
  - Runtime-scoped Worker identity の routing key。
  - Browser-facing API への safe projection。
  - workspace visibility / policy / audit hook point。
- Runtime internal store / allocation registry と Backend `RuntimeRegistry` を混同しない。
- Worker metadata persistence や live allocation authority は Runtime 側の責務とし、Backend Registry は直接所有しない。

### Runtime identity / handle model

- Backend-facing Runtime identity は `runtime_id` を authority とする。
- Worker authority は `runtime_id + worker_id` とする。
- UI 表示用 `display_ref` は authority にしない。
- Registry は将来以下の runtime source を扱える shape にする。
  - embedded `worker_runtime::Runtime`。
  - remote Runtime process client。
  - existing local Worker/Pod compatibility adapter。
- この Ticket では embedded / remote 実 handle の実装は後続に残す。
- Existing local metadata projection は必要なら compatibility source として残すが、正規 Runtime authority として扱わない。

### Backend API boundary

- Browser-facing API は Runtime endpoint / token / socket path / session path / local metadata path を受け取らない。
- Browser-facing API は `runtime_id + worker_id` を authority として扱える shape にする。
- Existing `/api/workers` / `/api/hosts` / runtime list behavior は、新 Registry model へ段階移行できるよう整理する。
- v0 では既存 API の behavior を維持しつつ、内部 model を RuntimeRegistry に寄せてよい。
- New runtime-scoped endpoints を足すか、既存 endpoints を拡張するかは実装時に決めてよいが、ticket内で選んだ方針を記録する。

### Implementation target

- 主な対象は `crates/workspace-server/src/hosts.rs` と `crates/workspace-server/src/server.rs` とする。
- 既存 `WorkerRuntimeRegistry` / `LocalPodRuntime` / `LocalRuntimeBridge` 相当を、Backend Registry foundation と local compatibility source の境界に整理する。
- `LocalPodRuntime` という名前が正規 Runtime 実装に見える場合は、`LocalPodCompatibilitySource` / `LocalWorkerCompatibilityAdapter` 相当の名前へ寄せる。
- 既存 local metadata reader の behavior は維持してよいが、Runtime authority ではなく compatibility projection として diagnostics / implementation kind に表す。
- 既存 `/api/hosts` / `/api/workers` / `/api/hosts/{host_id}/workers` の outward behavior は原則維持する。
- runtime-scoped endpoint を新設する場合は、既存 endpoint を壊さず追加する。
- この Ticket では Worker create / send input / remote HTTP call / embedded direct call の実処理は実装しない。後続 handle が差し込める型・routing境界までに留める。

### Error / diagnostics

- Unknown runtime、unknown worker、runtime unavailable、operation unsupported、worker not visible を typed に分けられるようにする。
- Compatibility local source 由来の stale metadata / invalid metadata は diagnostic として扱い、Runtime authority を歪めない。

## Non-goals

- Embedded `worker_runtime::Runtime` の登録・routing 実装。
- Remote HTTP Runtime client 実装。
- REST command server / event stream server implementation。
- Backend internal Companion Web Console completion。
- Dynamic Runtime registration。
- Full auth / permission model。
- Removing local compatibility path。

## 受け入れ条件

- Workspace backend に `worker-runtime` domain model と整合した `RuntimeRegistry` 基盤がある。
- Registry は Runtime identity / Worker routing key / capability / status projection を扱える。
- Registry の責務が Runtime internal store/allocation と code/docs/tests 上で分離されている。
- Browser-facing API が Runtime endpoint / token / socket path / session path を authority として受け取らない。
- Existing local Worker/Pod metadata projection は compatibility source として明示されている。
- Embedded/remote runtime 実装は後続 Ticket で追加できる handle boundary がある。
- Focused workspace-server tests cover Registry identity/projection/error mapping and local compatibility source behavior.
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
