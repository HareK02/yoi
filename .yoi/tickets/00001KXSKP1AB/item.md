---
title: 'Add development process switch script'
state: 'closed'
created_at: '2026-07-18T03:17:55Z'
updated_at: '2026-07-18T08:32:27Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-18T03:18:16Z'
---

## 背景

開発中の backend / runtime / workspace frontend は、手動で `cargo run` や `deno` task を main worktree から起動している。作業ブランチを切った worktree に付け替える時、既存 process を安全に止めて同じ port で起動し直す手順が必要になる。

この worker 自体が同じ開発 process 群に依存している可能性があるため、実装時には stop/restart/start の実行は行わず、静的検証に留める。

## 要件

- `scripts/dev-workspace.sh` を追加する。
- 引数で `start` / `stop` / `restart` を選べる。
- `start` は runtime / backend / frontend をこの checkout から起動する。
- `stop` は runtime / backend / frontend を停止する。
- `restart` は frontend を触らず、runtime / backend だけ停止・起動する。
- frontend は `0.0.0.0` bind で起動する。
- pid/log は repository-local な runtime directory に保存し、生成物は git 管理対象外にする。
- port listener の付け替えに対応する。

## 受け入れ条件

- `bash -n scripts/dev-workspace.sh` が通る。
- `scripts/dev-workspace.sh --help` が安全に使い方を表示する。
- start/stop/restart は実装するが、この作業中には実行しない。
- runtime/backend/frontend の command と default bind/port が script 内で確認できる。
