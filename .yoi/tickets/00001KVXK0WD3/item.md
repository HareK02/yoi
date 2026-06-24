---
title: 'Remove legacy raw WASM Plugin runtime'
state: 'inprogress'
created_at: '2026-06-24T19:51:56Z'
updated_at: '2026-06-24T20:14:56Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-24T20:11:56Z'
---

## 背景

Plugin platform roadmap は、public release 前に raw core-Wasm runtime を compatibility bridge として残す方針を撤回し、`wasm-component` / Component Model を正の Plugin runtime として一本化する。未公開機能に legacy layer を残すと、Manifest / WIT / PDK / runtime authority / Service execution model の境界が曖昧になる。

この Ticket は最初の実装 slice として、Pod runtime 内の legacy raw `wasm` execution path と `LegacyToolAdapter` を削除する。Manifest validation / CLI diagnostics の外向き rejection は別 Ticket で扱う。

## 要件

- `PluginInstanceRuntime::LegacyToolAdapter` 相当の active execution path を削除する。
- raw core-Wasm tool execution を Plugin instance runtime の active path から外す。
- `wasm-component` runtime を Plugin execution の唯一の active runtime path とする。
- legacy path 削除に伴い、不要になった adapter-only tests / fixtures / helper を削除または component runtime 用に更新する。
- Plugin Tool execution は Component Model path で維持する。
- Host API grant boundary、package discovery、enablement、digest pinning、ToolRegistry 登録の既存挙動を壊さない。

## Non-goals

- Manifest / CLI diagnostics で legacy runtime を拒否する外向き UX の完成。
- Service / Ingress event queue の実装。
- WebSocket driver の実装。
- WIT / PDK / templates の service event model 更新。
- Public registry / install / update policy。

## 受け入れ条件

- `LegacyToolAdapter` または同等の legacy raw-Wasm adapter が active runtime から削除されている。
- raw `wasm` runtime を通じた Plugin Tool execution path が使われていない。
- Component Model Plugin Tool execution の既存 tests が通る。
- Legacy runtime 削除に伴う dead code / dead tests / obsolete fixtures が整理されている。
- Plugin discovery / enablement / Tool registration / grant validation の既存 tests が通る。
- `cargo test -p pod`、`cargo check -p yoi`、`git diff --check`、`nix build .#yoi --no-link` が通る。
