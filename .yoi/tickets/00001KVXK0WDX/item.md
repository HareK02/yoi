---
title: 'Add Plugin service output command model'
state: 'inprogress'
created_at: '2026-06-24T19:51:56Z'
updated_at: '2026-06-25T05:47:43Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-24T20:12:02Z'
---

## 背景

Service / Ingress Plugin が外部 event を処理した後、WebSocket send、HTTP request、diagnostic update などの side effect を直接 ambient authority で実行すると、grant boundary と observability が曖昧になる。Plugin は event handler の戻り値として output commands を返し、Host が manifest declaration / enablement grant / runtime policy を検査して実行する形にしたい。

この Ticket では WebSocket driver 実装前に、Service Plugin の output command envelope と grant check 境界を追加する。

## 要件

- `handle-ingress` / service event handler の戻り値に output command list を表現できる型を追加する。
- v0 command kind を最小集合で定義する。
  - diagnostic/status update。
  - host request dispatch placeholder。
  - websocket send placeholder。
- Command は correlation id / source event id / command id / kind / payload / requested_at を持つ。
- Host が command ごとに manifest declaration と enablement grant を検査する。
- Unsupported / ungranted / malformed command は fail closed で diagnostic に残す。
- Command execution result は service status / run overview / diagnostics から追える形にする。
- Tool Plugin の ordinary ToolOutput path と混同しない。

## Non-goals

- WebSocket send の実 transport 実装。
- HTTP request dispatch command の完全実装。
- Domain operation command の完成。
- LLM history への hidden context injection。
- Unrestricted shell / filesystem command。

## 受け入れ条件

- Service Plugin ingress handler が output command envelope を返せる。
- Host が output command を parse / validate / grant-check する。
- Ungranted command は実行されず、typed diagnostic になる。
- Diagnostic/status update command のような safe v0 command が実行または記録される。
- WebSocket send / request dispatch は placeholder command として grant-check 可能で、実 transport が無くても安全に unsupported として扱える。
- Tool Plugin output と Service Plugin output command が型・docs・testsで区別されている。
- `cargo test -p pod`、`cargo check -p yoi`、`git diff --check`、`nix build .#yoi --no-link` が通る。
