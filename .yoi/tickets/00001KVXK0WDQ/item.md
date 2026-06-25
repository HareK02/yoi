---
title: 'Define Plugin Service lifecycle and ingress queue runtime'
state: 'closed'
created_at: '2026-06-24T19:51:56Z'
updated_at: '2026-06-25T14:13:52Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-24T20:12:00Z'
---

## 背景

Current Plugin instance support exposes `start` / `handle-ingress` / `status` / `stop`, but the execution model is still effectively synchronous function invocation. Service Plugins need a host-managed lifecycle and event queue so `start()` can return promptly, while external events are later delivered to `handle-ingress` without requiring a Plugin-owned polling loop.

This Ticket implements the first service runtime slice: lifecycle state, ingress queue, serial dispatch, backpressure, timeout, and failure semantics. WebSocket-specific event sources and output commands are handled by later Tickets.

## 要件

- Service Plugin lifecycle を host-managed state として扱う。
  - ready / starting / running / stopping / stopped / failed 相当。
- `start()` は initialization only とし、long-running loop / polling loop / recv loop を担わない。
- Service Plugin ごとに bounded ingress queue を持つ。
- Ingress event は source / ingress name / payload / created_at / attempt / correlation id を持つ。
- v0 dispatch は per-plugin serial dispatch とする。
- dispatch timeout、Plugin failure、queue full、invalid event、stop 中 event の扱いを typed にする。
- queue / lifecycle / failure state は status diagnostics として取得できる。
- Existing Tool Plugin execution は service queue に巻き込まず、request-response operation として維持する。

## Non-goals

- WebSocket connection driver の実装。
- Plugin output command model の本実装。
- Discord / Slack など特定 integration。
- Concurrent per-plugin event execution。
- Durable cross-process event queue。

## 受け入れ条件

- Service Plugin instance が host-managed lifecycle state を持つ。
- `start()` が返った後でも ingress event を queue 経由で配送できる。
- Queue は bounded で、full / timeout / failed service が typed error / diagnostic として扱われる。
- 同一 Plugin instance への ingress dispatch は serial に処理される。
- `status()` または host diagnostics から lifecycle state / queue depth / last error / dispatch counters を確認できる。
- Service lifecycle / ingress queue の unit tests が追加されている。
- Tool Plugin execution tests が regress しない。
- `cargo test -p pod`、`cargo check -p yoi`、`git diff --check`、`nix build .#yoi --no-link` が通る。
