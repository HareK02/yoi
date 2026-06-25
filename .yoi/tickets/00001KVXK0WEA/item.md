---
title: 'Update Plugin WIT PDK templates for service event runtime'
state: 'inprogress'
created_at: '2026-06-24T19:51:56Z'
updated_at: '2026-06-25T07:44:48Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-24T20:12:05Z'
---

## 背景

Runtime implementation が Component Model only、host-managed Service lifecycle、Ingress queue、output command、host-owned WebSocket driver へ移るなら、WIT / PDK / templates / docs もその実行モデルを正として表現する必要がある。PDK が long-running `start()` loop や explicit `recv(timeout)` polling を推奨すると、runtime 方針と authoring UX がズレる。

この Ticket は implementation runtime が揃った後、外部 authoring API を新 model に合わせて更新する仕上げ slice とする。

## 要件

- WIT world / interfaces を新しい Service lifecycle / Ingress event / output command model に合わせて更新する。
- PDK API は `start()` で long-running loop を書かせない形にする。
- PDK は ingress event handler から output command を返す authoring model を提供する。
- WebSocket integration template は Host-owned subscription / ingress event / send command pattern を示す。
- Tool template は bounded request-response Tool Plugin として維持する。
- `plugin.toml` template は `wasm-component` runtime のみを生成する。
- `yoi plugin new` / `check` / `pack` が新 templates / schema と整合する。
- Docs は legacy raw `wasm` runtime、polling WebSocket loop、ambient authority を推奨しない。

## Non-goals

- Runtime internal queue / WebSocket driver の本実装。
- Discord-specific PDK abstraction。
- Public plugin registry。
- MCP bridge integration。

## 受け入れ条件

- WIT files が Component Model only runtime と Service event/command model を表現している。
- `yoi-plugin-pdk` が Tool Plugin と Service/Ingress Plugin の新 API を提供する。
- Templates は legacy raw `wasm` runtime を生成しない。
- Service/WebSocket template は polling `recv(timeout)` loop ではなく ingress event / output command pattern を使う。
- `yoi plugin new rust-component-tool` と service-oriented template が check/pack できる。
- Development docs が新 authoring model と authority boundary を説明している。
- `cargo test -p yoi-plugin-pdk`、`cargo test -p yoi`、`cargo check -p yoi`、`git diff --check`、`nix build .#yoi --no-link` が通る。
