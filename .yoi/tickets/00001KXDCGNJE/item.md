---
title: 'Remove workspace and cwd from worker runtime server'
state: 'inprogress'
created_at: '2026-07-13T09:21:46Z'
updated_at: '2026-07-13T09:22:30Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-13T09:22:30Z'
---

## 背景

`worker-runtime-rest-server` は remote Runtime process として backend から URL で操作される。Workspace authority は workspace-server / backend 側が提供するため、Runtime process が `--workspace` / `--cwd` を受け取り、自身で workspace root や cwd を推測する必要はない。

現在の `--workspace` / `--cwd` は、古い local workspace 前提の名残であり、profile source archive / backend resource / working-directory binding による authority 境界と合っていない。

## 要件

- `worker-runtime-rest-server` の `--workspace` と `--cwd` option を削除する。
- Runtime-local Worker root は workspace ではなく Runtime-local durable/scratch root から導出する。
- Workdir が materialize された Worker だけが materialized Workdir root/cwd を受け取る。
- Workdirless Worker は workspace filesystem authority を持たないまま起動する。
- Backend-provided profile source / resource endpoint / working-directory binding の経路は維持する。

## 受け入れ条件

- `--workspace` / `--cwd` は help から消え、指定すると unknown argument として reject される。
- `worker-runtime-rest-server` は `--workspace` なしで起動できる。
- `cargo test -q -p worker-runtime --features fs-store,ws-server` が通る。
- `cargo test -q -p yoi-workspace-server` が通る。
