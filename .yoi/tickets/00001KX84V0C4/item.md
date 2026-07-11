---
title: 'Preserve requested Worker display names'
state: 'inprogress'
created_at: '2026-07-11T08:31:24Z'
updated_at: '2026-07-11T08:31:54Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-11T08:31:54Z'
---

## 背景

New Worker 画面は既定で `Coding Worker` などの display name を送るが、Workspace Backend が Worker 作成後の registry 同期で Runtime projection の `worker_id` 由来 label を display name として保存している。その結果 Workers list の label と id がどちらも `worker-00000001` のように見える。

Display name は UI/request の値を preserving metadata として扱い、Runtime worker id とは分ける必要がある。

## 要件

- Worker 作成時に `requested_worker_name` / UI display name を Backend worker registry の `display_name` として保存する。
- 以後の Runtime observation/list sync で既存 registry の `display_name` を Runtime id label で上書きしない。
- registry record が無い live Worker だけ Runtime projection label を fallback display name として使う。
- Workers list の1カラム目は display name、補助表示として worker id を別に出す。

## 受け入れ条件

- New Worker 画面から `Coding Worker` で作成した Worker が Workers list で `Coding Worker` と表示される。
- 同じ行で `worker-00000001` 等の Worker id は metadata として別に確認できる。
- Runtime list sync 後も display name が worker id に戻らない。
- `cargo test -q -p yoi-workspace-server`、`cd web/workspace && deno task check`、`cd web/workspace && deno task test`、`nix build .#yoi` が通る。
