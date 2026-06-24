---
title: 'Reject legacy Plugin runtime in manifest and CLI diagnostics'
state: 'queued'
created_at: '2026-06-24T19:51:56Z'
updated_at: '2026-06-24T20:52:56Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-24T20:11:58Z'
---

## 背景

Plugin runtime は public release 前に `wasm-component` へ一本化する。runtime implementation から legacy raw `wasm` path を削除しても、Manifest parser / validator / `yoi plugin check/list/show` が legacy runtime を曖昧に受け付けると、ユーザー向け API と実装方針がズレる。

この Ticket では、legacy runtime を外向き schema / diagnostics でも明示的に拒否し、docs / fixtures から transitional wording を消す。

## 要件

- `plugin.toml` の runtime validation で legacy raw `wasm` / `yoi-plugin-wasm-1` を新規 Plugin runtime として受け付けない。
- `runtime.kind = "wasm-component"` を正の runtime kind として扱う。
- `yoi plugin check` は legacy runtime を invalid / unsupported として分かりやすく報告する。
- `yoi plugin list/show` は legacy package を有効 Plugin として曖昧に表示しない。
- docs/design / docs/development / templates / examples から「raw core-Wasm compatibility bridge を残す」前提の記述を削除または撤回済み方針に更新する。
- Manifest schema / static inspection / error message tests を更新する。

## Non-goals

- Runtime internal の legacy adapter 削除そのもの。
- Service / Ingress event runtime の実装。
- WebSocket driver の実装。
- PDK API の全面更新。

## 受け入れ条件

- Legacy raw `wasm` runtime manifest が validation で拒否される。
- `yoi-plugin-wasm-1` ABI は current public/recommended Plugin API として表示されない。
- `yoi plugin check/list/show` が legacy runtime package を安全かつ明確に diagnostic する。
- Component Model package の check/list/show は regress しない。
- Plugin objective / design / development docs が Component Model only 方針と整合している。
- Legacy runtime 前提の fixtures が削除または rejected fixture として更新されている。
- `cargo test -p manifest`、`cargo test -p yoi`、`cargo check -p yoi`、`git diff --check`、`nix build .#yoi --no-link` が通る。
