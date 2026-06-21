---
title: 'Workspace web control plane bootstrap'
state: 'inprogress'
created_at: '2026-06-21T06:57:06Z'
updated_at: '2026-06-21T07:44:49Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-21T07:11:58Z'
---

## 背景

Objective `00001KVJPT2PP`（Team workspace control plane and runner architecture）の最初の実装スライスとして、Web から扱える Workspace control plane の土台を作る。

方針:

- フルスタック framework ではなく、Rust backend + 静的 SPA frontend にする。
- frontend は当面 monorepo 内に置く。置き場所は実装時に選ぶが、backend packaging / Nix build / repository hygiene を壊さない場所にする。
- backend は Workspace ごとの authority boundary として設計する。
- 初期実装は single-workspace server でよい。ただし record / API / store 設計は将来の multi-workspace / hosted deployment を阻害しない。
- DB は最初 SQLite を使う。
- backend と store 層は抽象化する前提で作り、SQLite 実装を最初の store backend とする。
- Memory / Knowledge の本格再設計はこの Ticket では扱わない。将来の Workspace backend 移行時に回収する。

## 要件

### Backend crate

- Workspace control plane backend 用の Rust crate を追加する。
  - crate 名は実装時に決めてよいが、候補は `workspace-server` / `workspace` / `control-plane`。
  - public product command は将来的に `yoi` crate 側から露出する前提にし、backend crate に product CLI ownership を散らさない。
- backend crate は HTTP API、static SPA serving、event stream / runner connection の将来拡張点を持つ。
- 初期 server は single-workspace mode で起動できる。
- API/state の内部 model には `workspace_id` を含め、将来の multi-workspace hosting に備える。

### Store abstraction

- Store 層は trait / interface 境界を持ち、SQLite 実装に直接結合しすぎない。
- 初期 SQLite store を追加する。
  - WAL、foreign keys、busy timeout など、SQLite server use に必要な基本設定を行う。
  - migration / schema versioning の最小方針を持つ。
- 初期 schema は最小限でよいが、少なくとも Workspace / Ticket / Objective / Repository / Run / Artifact / Runner の後続拡張を想定できる配置にする。
- 既存 `.yoi/tickets` / `.yoi/objectives` は当面 canonical 互換 backend として残る。必要なら import/bridge は follow-up とし、この Ticket で無理に完全移行しない。

### Frontend

- SvelteKit static SPA の skeleton を monorepo 内に追加する。
- frontend は static build を前提にし、SSR に backend authority を持たせない。
- Rust backend は build 済み static assets を serve できる。
- SPA routing fallback と `/api/...` の分離を設計する。
- frontend package manager / lockfile / Nix packaging の扱いを明確にする。

### Initial API / UX slice

- 最初は read-heavy skeleton でよい。
- 候補 API:
  - `GET /api/workspace`
  - `GET /api/tickets`
  - `GET /api/tickets/{id}`
  - `GET /api/objectives`
  - `GET /api/objectives/{id}`
  - `GET /api/runs`
  - `GET /api/runners`
- write API、runner job dispatch、Ticket state mutation、Memory migration は follow-up でよい。
- auth は初期 local/dev token または explicit local-only mode でよいが、multi-user SaaS 前提と衝突する形にしない。

## Non-goals

- full hosted SaaS / multi-tenant production server。
- cloud runner fleet / scheduling / billing / quota。
- Ticket/Objective の `.yoi` canonical store 完全移行。
- Memory / Knowledge の本格再設計。
- Git hosting service の実装。
- frontend に business logic / lifecycle authority を持たせること。
- Desktop app。

## 受け入れ条件

- Workspace control plane backend 用 crate が workspace に追加されている。
- backend と store 層の境界があり、SQLite はその初期実装として使われている。
- SQLite schema / migration/versioning の最小実装がある。
- single-workspace server を起動でき、静的 SPA と `/api/...` を同じ backend から serve できる。
- SvelteKit static SPA skeleton が monorepo 内に追加されている。
- frontend build artifact の扱いが Nix/package build で破綻しない。
- 初期 read API が少なくとも workspace/ticket/objective の bounded JSON を返す。
- existing local `.yoi` record workflow を壊さない。
- `cargo fmt --check`、関連 `cargo test` / `cargo check`、frontend の build/check、`git diff --check`、`yoi ticket doctor`、`yoi objective doctor`、`nix build .#yoi --no-link` が通る。
