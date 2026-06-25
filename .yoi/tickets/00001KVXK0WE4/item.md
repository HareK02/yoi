---
title: 'Add host-owned WebSocket driver for Plugin services'
state: 'queued'
created_at: '2026-06-24T19:51:56Z'
updated_at: '2026-06-25T06:22:18Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-24T20:12:03Z'
---

## 背景

Current `host_api.websocket` is a pull primitive: Plugin code must explicitly call `recv(timeout)` while an exported function is running. That is insufficient for Discord Gateway / Slack Socket Mode style long-lived integrations. Service Plugins need Host-owned WebSocket connections whose incoming frames are queued as ingress events and whose outgoing sends are requested through output commands.

This Ticket adds the WebSocket-specific event source and command executor on top of the Service ingress queue and output command model.

## 要件

- Host-owned WebSocket connection driver を追加する。
- Plugin manifest / grant で WebSocket subscription / target authority を表現できる。
- Host が connect / reader task / close / error detection を管理する。
- Incoming text frame を Plugin ingress event に変換し、service ingress queue に enqueue する。
- Close / error / reconnect-needed も ingress event または status diagnostic として扱う。
- Plugin からの WebSocket send は output command として受け取り、grant check 後に Host が送信する。
- v0 で扱う frame kind を明確にする。
  - text frame required。
  - binary / ping / pong / close の扱いを diagnostic / unsupported / control handling として定義する。
- Connection status、last frame time、last error、queue drops、send failures を diagnostics に出す。
- Existing `recv(timeout)` pull API は long-lived integration の recommended path にしない。

## Non-goals

- Discord protocol implementation。
- Full reconnect / resume policy の完成。
- Secret store / auth injection の全面設計。
- Binary frame application payload support。
- Browser-facing WebSocket API。

## 受け入れ条件

- Host-owned WebSocket driver が Service Plugin に紐づく connection を管理できる。
- Incoming text frame が Plugin ingress queue に event として入る。
- Plugin output command から WebSocket text send が実行される。
- Ungranted send / unauthorized target は fail closed で diagnostic になる。
- Connection close / error が service status diagnostics に反映される。
- Existing pull `recv(timeout)` API が docs/templates の recommended service integration path から外れている。
- WebSocket driver / ingress / send command の tests が追加されている。
- `cargo test -p pod`、`cargo check -p yoi`、`git diff --check`、`nix build .#yoi --no-link` が通る。
