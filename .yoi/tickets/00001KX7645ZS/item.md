---
title: 'Remove backend worker observation replay'
state: 'closed'
created_at: '2026-07-10T23:34:39Z'
updated_at: '2026-07-10T23:50:55Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-10T23:35:04Z'
---

## 背景

Workspace Console の worker observation WebSocket は、BackendObservationProxy の保存済み history を接続時に replay している。そのためページ読み込み直後に Snapshot の後で過去の `user_message` などが WS frame として流れ、Console では「過去イベントがストリームされている」ように見える。

Console attach は現在状態の Snapshot と以後の live observation を見たいのであって、backend-local replay は不要。Snapshot は bootstrap、以後の event は live のみとする。

## 要件

- workspace-server の worker observation WebSocket から backend-local replay を削除する。
- BackendObservationProxy は replay history を持たず、必要なら runtime cursor の引き継ぎだけを扱う。
- Snapshot を backend history として保存・再送しない。
- Frontend Console 接続時に、Snapshot 後に過去の `user_message` / `tool_result` 等が replay されない。
- Runtime から live に届く observation event は従来通り browser に転送される。

## 受け入れ条件

- worker observation WS 接続時、最初に Snapshot が1回届き、その後は接続後に発生した live events のみ届く。
- backend-local replay 用の history / cursor validation / cursor unknown handling が削除または無効化される。
- 既存の proxy WS tests を replay 前提から snapshot + live 前提に更新する。
- `cargo test` と `nix build .#yoi` が通る。
