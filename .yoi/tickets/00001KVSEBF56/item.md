---
title: 'Generate Workspace web TypeScript types from protocol crate'
state: 'inprogress'
created_at: '2026-06-23T05:13:22Z'
updated_at: '2026-06-23T05:41:25Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-23T05:40:01Z'
---

## 背景

Workspace web から Worker / Pod 操作 UI を実装するにあたり、Frontend と Backend が Pod protocol の wire 型を共有する必要がある。TypeScript 側で protocol 型を手書きすると Rust 側の `crates/protocol` と容易に乖離するため、Rust の serde DTO を authority として TypeScript 型を自動生成する。

方針:

- `crates/protocol` を Pod protocol の wire schema authority とする。
- Browser は Pod Unix socket に直接接続せず、Workspace backend が proxy する。
- Backend proxy は Worker identity 解決、socket 接続、stale/missing Worker の検出、必要最小限の method block/allow 判断を行う。
- 現時点では user permission model がないため、web 専用 protocol を過剰に多重化しない。Pod protocol の `Method` / `Event` を基本に扱い、危険または UI 対象外の操作だけ backend 側で止める。
- TypeScript 型共有の本線は `ts-rs` 等による自動生成とする。wasm 化は protocol crate の portability / optional codec 用であり、TS discriminated union 型生成の代替として扱わない。

## 要件

### protocol crate の transport 非依存化

- `crates/protocol` の DTO 部分を browser/wasm target でも compile できるようにする。
- `stream.rs` の `tokio::io` JSONL reader/writer は optional feature に分離する。
  - 例: default feature `stream`。
  - `tokio` dependency は `stream` feature 配下の optional dependency にする。
- `lib.rs` は `#[cfg(feature = "stream")] pub mod stream;` のようにし、`--no-default-features` では DTO のみが compile されるようにする。
- 少なくとも以下が通る状態にする。
  - `cargo check -p protocol --target wasm32-unknown-unknown --no-default-features`

### TypeScript 型自動生成

- `protocol` の主要 wire 型から Workspace web 用 TypeScript 型を自動生成する。
- 手書きの protocol mirror 型を増やさない。
- 第一候補は `ts-rs`。
  - `serde` attribute compatibility を使い、`#[serde(tag = ..., content = ..., rename_all = ...)]` の wire 表現と TS 型を合わせる。
  - `uuid::Uuid`、`serde_json::Value`、`PathBuf` などの扱いを明示する。必要なら feature や `#[ts(type = "...")]` / `#[ts(as = "...")]` を使う。
- 生成対象の root type は少なくとも以下を含める。
  - `Method`
  - `Event`
  - `Segment`
  - Worker 操作 UI / stream 表示で直接参照する関連 DTO
- 生成ファイルは Workspace web が import できる場所に置く。
  - 例: `web/workspace/src/lib/generated/protocol.ts` または同等の generated directory。
- 生成物の更新漏れを検出できる検証手段を用意する。
  - 例: cargo test / xtask / generator binary で生成し、差分が出たら失敗する check。

### Workspace backend proxy 方針

- Browser から Pod protocol を直接 socket に送らず、Workspace backend 経由にする設計にする。
- Backend は `worker_id` から local Pod identity/socket を解決する。
- Backend は proxy 時に最低限以下を確認する。
  - 対象 Worker が Workspace runtime view から見えること。
  - Pod socket が存在し接続可能であること。
  - UI/API から許可する `Method` であること。
  - stale/missing/disconnected Worker の error を明示的に返すこと。
- user permission model が入るまでは、別 protocol による権限モデルを先取りしない。

### Frontend integration stance

- Workspace web は generated TypeScript types を import して Worker 操作 UI / event handling に使う。
- Protocol 型の handwritten mirror を残す場合は、暫定互換や UI 専用 view model として明示し、wire type と混同しない。
- WebSocket/SSE/HTTP の transport 選択は後続実装で決めてよいが、wire payload 型は generated protocol types に寄せる。

## Non-goals

- Full Worker operation UI の完成。
- Full WebSocket session UI の完成。
- User / role / permission model の設計。
- Browser から Pod Unix socket へ直接接続する実装。
- Pod protocol の全面的な再設計。
- OpenAPI / JSON Schema への全面移行。

## 受け入れ条件

- `crates/protocol` の `tokio` stream 依存が optional feature 化されている。
- `cargo check -p protocol --target wasm32-unknown-unknown --no-default-features` が通る。
- `protocol` の主要 wire DTO から TypeScript 型が自動生成される。
- Workspace web が手書き mirror ではなく generated TypeScript 型を import できる。
- `serde` tagged enum / rename などの wire 表現と generated TS 型の整合性が検証されている。
- 生成物の更新漏れを検出する check がある。
- Backend proxy は browser request を Pod socket に直接丸投げせず、Worker identity 解決と最低限の method allow/block 判断を挟む方針が実装または設計記録に反映されている。
- 実装まで含める場合は `cargo test -p protocol`、`cargo check -p protocol --target wasm32-unknown-unknown --no-default-features`、`cd web/workspace && deno task check && deno task build`、`git diff --check`、`nix build .#yoi --no-link` が通る。
