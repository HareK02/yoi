---
title: 'Remove worker observation WebSocket cursor surface'
state: 'closed'
created_at: '2026-07-11T00:09:06Z'
updated_at: '2026-07-11T00:31:57Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-11T00:09:32Z'
---

## 背景

Workspace worker observation WebSocket には backend-local `cursor` / `event_id` surface が残っているが、backend replay を削除した後は resume cursor として意味を持たない。Console attach は Snapshot + live events のみで十分であり、未読 output 読み取りが必要な場合は WebSocket cursor ではなく durable output/log 専用 cursor を別設計する。

## 要件

- workspace-server の worker observation browser-facing WebSocket から backend-local cursor surface を削除する。
- `ClientWorkerEventsWsQuery.cursor` / `BackendObservationCursor` / backend `next_sequence` / `open()` / `store()` を削除または単純な envelope mapping に縮小する。
- browser-facing event envelope は Runtime/Worker identity と payload に限定し、存在するなら runtime event id のみを重複抑止用に渡す。
- Frontend Console は backend cursor に依存せず、runtime event id で dedupe する。
- cursor 付き WS 接続は API surface として提供しない。クエリを無視する設計にせず、不要な型・検証・診断を消す。

## 受け入れ条件

- worker observation WS response frame に backend-local cursor が含まれない。
- Frontend Console model / route が cursor を保持・利用しない。
- backend-local cursor 関連型と cursor malformed/unknown handling が削除される。
- Runtime observation cursor は runtime 内部または runtime endpoint だけの概念として残り、workspace-server browser surface には露出しない。
- `cargo test`、`cd web/workspace && deno task check`、`cd web/workspace && deno task test`、`nix build .#yoi` が通る。
