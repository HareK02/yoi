---
title: 'Plugin: enforce Plugin permission grants'
state: 'inprogress'
created_at: '2026-06-15T14:48:59Z'
updated_at: '2026-06-18T13:11:51Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'permission', 'grant-enforcement', 'capability-boundary', 'tool-execution']
queued_by: 'workspace-panel'
queued_at: '2026-06-18T13:11:00Z'
---

## Background

Plugin package discovery / Tool registration / Tool runtime が揃った後、Plugin manifest の requested permissions と Profile/config 側の granted permissions を照合し、登録・実行・host API 利用の境界で fail closed にする。

この Ticket では `https` / `fs` host API の実装自体は行わない。ただし後続の host API 実装が同じ grant model を使えるよう、grant の型と enforcement point を明確にする。

## Requirements

- Plugin manifest の requested permissions を typed data として読む。
  - enabled surfaces
  - tool names / namespaces
  - Tool effect / external_write metadata
  - future host APIs: `https`, `fs`
- Profile/config 側の granted permissions を typed data として読む。
  - package ref / digest / version と結びつける。
  - source-qualified identity に対応する。
- Requested permissions と granted permissions を照合する。
  - requested but not granted は fail closed。
  - unsupported grant / unknown permission kind は fail closed。
  - overly broad ambiguous grant は fail closed または明示 diagnostic。
- Enforcement points を実装する。
  - Plugin package enablement resolution
  - Tool surface registration
  - Plugin Tool execution
  - future host API call dispatch
- Tool metadata の effect を existing permission / PreToolCall path と整合させる。
  - external side effect がある Tool は metadata と permission gate に反映する。
  - external write は `Outbound` surface ではなく Tool metadata として扱う。
- Denied reason を diagnostic / trace で確認できるようにする。
  - grant missing
  - grant digest mismatch
  - requested surface not granted
  - requested tool not granted
  - host API not granted
  - external_write not granted
- Grant enforcement は model-visible context に隠し情報を差し込まない。
- Package が存在するだけ、または Tool registration だけで execution authority を得ない。

## Initial grant model guidance

最初の grant model は狭く始める。

```text
surfaces.tool
tool names / namespaces
external_write flag
host_api.https
host_api.fs
```

この Ticket では `https` / `fs` の API 実行は non-goal。ただし requested/granted の型と denied diagnostic は先に扱えるようにする。

## Acceptance criteria

- Grant なしの Plugin Tool は実行されず、safe diagnostic になる。
- Granted Tool だけが登録または実行可能になる。
- Requested surface が grant に含まれない場合は fail closed する。
- Requested external_write が grant に含まれない場合は fail closed する。
- Digest/version/source mismatch の grant は使われない。
- Unknown permission kind / unsupported grant は fail closed する。
- Denied reason が diagnostic / trace で確認できる。
- Existing PreToolCall / Tool permission path と矛盾しない。
- Tests cover:
  - no grant denies Plugin Tool execution
  - grant allows specific Plugin Tool
  - unrelated package grant does not apply
  - digest mismatch denies
  - requested surface missing denies
  - external_write missing denies
  - unknown permission kind fails closed
  - denial reason is bounded and safe
- Validation: focused plugin permission tests, `cargo fmt --check`, relevant `cargo check` / `cargo test`, `git diff --check`.

## Non-goals

- Implementing `https` host API.
- Implementing `fs` host API.
- Service / Ingress surface.
- Plugin package manager / install/update.
- Signature/trust-chain enforcement.
- Broad policy UI.

## Related work

- `00001KV5R5V2S` — Plugin package discovery and explicit enablement resolver.
- `00001KV5W3PHA` — Plugin Tool surface registration.
- `00001KV5W3PHW` — Plugin Tool execution with minimal WASM runtime.
- `00001KSXRQ4G8` — Plugin runtime / surface / host API model design.
