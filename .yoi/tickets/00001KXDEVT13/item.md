---
title: 'Normalize worker runtime filesystem options'
state: 'closed'
created_at: '2026-07-13T10:02:48Z'
updated_at: '2026-07-13T12:05:03Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-13T10:04:08Z'
---

## 背景

`worker-runtime-rest-server` の filesystem option は、`fs-store` feature によって使える永続化境界として見えるべきである。現在の `--worker-store-dir` / `--worker-metadata-dir` / `--worker-runtime-base-dir` は内部実装の分割を直接露出しており、`--store memory` も「store backend」ではなく「永続 store を使わない ephemeral mode」を表している。

`fs-store` feature 付き build では filesystem-backed store が default で動き、明示的に永続化を切る場合だけ `--no-store` とする。

## 要件

- `--fs-root` は filesystem-backed storage 全体の top-level root とする。
- `--fs-worker-dir` を追加し、Worker session / metadata / controller runtime / workdirless Worker root をまとめて導出する。
- `--fs-runtime-dir` を追加し、Runtime catalog / Worker list / execution mapping / events の保存先にする。
- `--workdir-target` を追加し、Workdir materialization target を指定できるようにする。
- `--store <fs>` は future backend switching 用に残し、`memory` は受け付けない。
- 旧 `--store memory` は `--no-store` に置き換える。
- 旧 `--worker-store-dir` / `--worker-metadata-dir` / `--worker-runtime-base-dir` は削除する。

## 受け入れ条件

- `--help` に新しい option 名が表示され、旧 worker path option は表示されない。
- `--store memory` は reject され、`--no-store` は Runtime catalog persistence を無効化する。
- `--fs-root <dir>` だけで `<dir>/runtime`、`<dir>/worker`、`<dir>/workdirs` が default 派生される。
- Worker Runtime server は新 option で起動できる。
- worker-runtime / workspace-server の関連テストと Nix build が通る。
