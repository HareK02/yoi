---
title: 'Remove durable worker lifecycle_state'
state: 'inprogress'
created_at: '2026-07-11T04:08:12Z'
updated_at: '2026-07-11T04:09:06Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-11T04:09:06Z'
---

## 背景

Backend worker registry は `lifecycle_state` を永続化しているが、`running` / `idle` / `stopped` などの Worker lifecycle は Runtime が authority を持つ live state であり、Backend DB に保存すると Runtime 再起動後に stale な state が Workers list に残る。Registry は Worker identity / refs / retention を保存し、live state は Runtime 問い合わせまたは短命 cache に限定する。

## 要件

- `WorkerRegistryRecord` と `worker_registry` schema から `lifecycle_state` を削除する。
- worker registry upsert/select/read の SQL から `lifecycle_state` を削除する。
- registry-only Worker projection は過去の lifecycle state を status/state として返さず、`archived` / `registry_only` 等の live ではない状態として表現する。
- live Worker が存在する場合は Runtime 由来の status/state をそのまま使い、registry metadata は label/profile/retention/working-directory refs などに限定して merge する。
- 既存 DB に `lifecycle_state` column が残っていても読み書きが壊れないよう、active schema/access path はその column に依存しない。

## 受け入れ条件

- `WorkerRegistryRecord` に `lifecycle_state` field が存在しない。
- `worker_registry` の active CREATE TABLE / INSERT / SELECT / read path に `lifecycle_state` が存在しない。
- `merge_worker_registry_projection(Some(live), ...)` が live status/state を registry value で上書きしない。
- `worker_summary_from_registry()` が `idle` / `running` / `stopped` など過去の live lifecycle を返さない。
- Workspace server tests が registry-only Worker と live Worker merge の両方を検証する。
- `cargo test` と `nix build .#yoi` が通る。
