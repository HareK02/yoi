---
title: 'Plugin: register enabled Tool surface from packages'
state: 'ready'
created_at: '2026-06-15T14:48:59Z'
updated_at: '2026-06-15T14:50:28Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'tool-registry', 'model-visible-schema', 'capability-boundary', 'profile-config']
---

## Background

Plugin package discovery / explicit enablement resolver の次に、enabled Plugin package から Tool surface を読み取り、通常の `ToolRegistry` に登録できるようにする。

この Ticket の目的は、Plugin package 由来の Tool 定義が Yoi の既存 Tool 経路に安全に乗る境界を作ること。Plugin code execution / WASM runtime はまだ行わない。Tool が model-visible schema として見えるか、enablement なしでは出ないか、invalid / duplicate な Tool 定義が fail closed になるかを先に固める。

## Requirements

- `00001KV5R5V2S` の resolved Plugin metadata を入力として扱う。
- Enabled Plugin package の manifest から Tool surface 定義を読み取る。
  - tool name
  - description
  - input schema
  - effect / side-effect metadata
  - plugin origin metadata
- Plugin Tool definition を既存 `ToolRegistry` 登録経路に載せる。
  - model-visible schema は通常 Tool と同じ原則に従う。
  - feature/profile config で disabled なら schema surface から消える。
- Tool metadata に Plugin origin を保持する。
  - plugin id / ref
  - package source: user / project / builtin
  - package digest
  - package version / api version
  - surface: tool
- Duplicate Tool name は fail closed にする。
  - builtin Tool / other Plugin Tool との衝突を検出する。
  - どちらが勝つかを曖昧にしない。
- Invalid input schema / unsupported schema shape は fail closed にする。
- Package が discovered されただけでは Tool を登録しない。
  - explicit enablement が必要。
- Tool call / result は後続 runtime Ticket で実装する。
  - この Ticket では未実行 Tool として registration boundary を作る。
  - 実行できない状態を user-visible diagnostic として安全に扱う。
- Diagnostics は bounded にする。
  - registered
  - skipped: not enabled
  - rejected: duplicate name
  - rejected: invalid schema
  - rejected: unsupported surface/api
  - rejected: missing runtime executor

## Acceptance criteria

- Enabled Plugin package の Tool definition が `ToolRegistry` に登録され、model-visible tools に現れる。
- Enablement がない Plugin package の Tool は model-visible tools に現れない。
- Duplicate Tool name は登録されず、diagnostic で理由が分かる。
- Invalid input schema は登録されず、diagnostic で理由が分かる。
- Registered Plugin Tool の metadata から plugin origin / digest / source が追跡できる。
- Feature/profile flag により Plugin Tool surface を非表示にできる。
- Tool call がまだ実行できない場合も panic せず、安全な unavailable/runtime-missing error になる。
- Tests cover:
  - enabled package Tool registration
  - package without enablement does not register
  - duplicate Plugin Tool name rejected
  - builtin Tool name collision rejected
  - invalid schema rejected
  - plugin origin metadata retained
  - disabled feature/profile removes schema surface
- Validation: focused plugin/tool-registry tests, `cargo fmt --check`, relevant `cargo check` / `cargo test`, `git diff --check`.

## Non-goals

- Plugin code execution.
- WASM runtime.
- `https` / `fs` host API.
- Service / Ingress surface.
- External side effects.
- Permission grant enforcement beyond registration-time shape checks.

## Related work

- `00001KV5R5V2S` — Plugin package discovery and explicit enablement resolver.
- `00001KV5W3PHW` — Plugin Tool execution with minimal WASM runtime.
- `00001KV5W3PJ3` — Plugin permission grant enforcement.
- `00001KSXRQ4G8` — Plugin runtime / surface / host API model design.
