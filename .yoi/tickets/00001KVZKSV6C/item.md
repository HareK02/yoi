---
title: 'Backend RuntimeRegistryをworker-runtimeへ接続する'
state: 'planning'
created_at: '2026-06-25T14:44:03Z'
updated_at: '2026-06-25T14:47:43Z'
assignee: null
---

## 背景

Workspace Backend は複数 Runtime を束ねる `RuntimeRegistry` を持つ。Registry は Worker を実行する主体ではなく、embedded Runtime と remote Runtime process を同じ logical operation で扱うための集約境界である。`worker-runtime` core / REST command server / event stream server が揃ったら、Backend は embedded Runtime には direct lib call、remote Runtime には HTTP client / event stream client で接続できるようにする必要がある。

この Ticket では Workspace backend の既存 local Pod metadata projection 寄りの Runtime/Worker handling を、`worker-runtime` の embedded/remote Runtime handles に接続する。

## 要件

- `worker-runtime` に `http-client` feature または Backend-local remote Runtime client を追加する。
- Backend RuntimeRegistry は少なくとも以下の handle を扱える。
  - embedded Runtime handle。
  - remote HTTP Runtime client handle。
  - existing local Pod compatibility adapter は必要なら別 branch として残す。
- Backend API は Browser から Runtime endpoint / credential / socket path / session path を受け取らない。
- Browser-facing API は `runtime_id + worker_id` を authority とする。
- Backend は Runtime config / endpoint / token secret ref / capability cache を管理する。
- Backend は Runtime の command API を proxy し、policy / visibility / audit / typed error mapping を挟む。
- Backend は Runtime event stream を購読または proxy できる形にする。
- Existing `/api/workers` / `/api/hosts` / runtime list behavior を new Runtime model へ段階移行する。
- Local Pod metadata reader は正規 Runtime ではなく compatibility adapter として扱う。

## Non-goals

- `worker-runtime` core crate implementation。
- REST command server implementation。
- Event stream server implementation。
- Backend internal Companion Web Console completion。
- Dynamic Runtime registration。
- Full auth / permission model。
- Removing local Pod compatibility path。

## 受け入れ条件

- Workspace backend can register an embedded `worker_runtime::Runtime`.
- Workspace backend can register a remote Runtime client from config-like data.
- Backend RuntimeRegistry routes list/detail/input/transcript/event operations to embedded or remote Runtime handles.
- Browser-facing API does not expose Runtime credentials, raw endpoint authority, socket path, or session path.
- Existing worker list/detail endpoints continue to work or are explicitly replaced with new runtime-scoped endpoints.
- Local Pod metadata projection is clearly marked as compatibility adapter, not the Runtime authority.
- Focused workspace-server tests cover embedded Runtime and mocked remote Runtime routing.
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `git diff --check` が通る。
- `nix build .#yoi --no-link` が通る。
