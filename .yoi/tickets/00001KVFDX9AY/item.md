---
title: 'Plugin: implement fs host API for Tool runtime'
state: 'queued'
created_at: '2026-06-19T07:53:13Z'
updated_at: '2026-06-19T10:22:26Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'fs', 'host-api', 'sandbox', 'path-safety', 'permission-grants', 'file-mutation']
queued_by: 'workspace-panel'
queued_at: '2026-06-19T10:19:52Z'
---

## Background

Plugin Tool runtime は minimal WASM execution と permission grants まで実装済みだが、Plugin-layer scoped filesystem access はまだ未実装である。

この Ticket では、WASM Plugin Tool から明示 grant された scoped paths のみを read/list/write できる `fs` host API を追加する。Plugin は Pod / workspace の filesystem authority を自動継承しない。Plugin-specific grant だけが有効な authority になる。

## Requirements

- WASM Plugin Tool runtime に `fs` host API import を追加する。
  - API 名・ABI は既存 `yoi-plugin-wasm-1` / host import 設計と整合させる。
  - Plugin は ambient filesystem access を持たず、host API 経由のみで fs operation できる。
- Plugin-layer scoped paths を grant で表現する。
  - read
  - list
  - write の初期 subset
  - optional path root / glob / prefix policy は implementation-time に最小安全形を選ぶ。
- Workspace filesystem scope を自動継承しない。
  - Pod が workspace write authority を持っていても Plugin は grant なしでは読めない/書けない。
- Path safety を徹底する。
  - normalization
  - `..` traversal reject
  - symlink/root escape reject
  - absolute/relative path policy を明確化
  - allowed root 外は fail closed
- Bounds を設ける。
  - read size bound
  - write size bound
  - directory entry count bound
  - path length bound
  - diagnostic size bound
- Writes は既存 file mutation safety と整合させる。
  - normalized target file ごとの serialization / atomic-ish behavior を検討する。
  - broad Worker scheduler は追加しない。
- Diagnostics は safe にする。
  - file content を error/log に漏らさない。
  - rejected path は必要最小限にする。
- Tool result path は通常 Tool result/history 経路を使う。
  - hidden context injection しない。

## Acceptance criteria

- Granted Plugin Tool can read an allowed file through `fs` host API.
- Granted Plugin Tool can list an allowed directory within bounds.
- Granted Plugin Tool can write an allowed file within bounds.
- Plugin without matching `host_api.fs` grant cannot read/list/write.
- Workspace write authority is not inherited by Plugin without Plugin grant.
- `../` traversal, symlink escape, and allowed-root escape are rejected.
- Oversize read/write/list results fail closed or truncate according to explicit policy.
- File mutation safety does not race unsafely with existing Write/Edit semantics.
- Diagnostics do not include file content or secret-like data.
- Tests cover:
  - allowed read
  - allowed list
  - allowed write
  - missing grant denied
  - workspace authority not inherited
  - path traversal rejected
  - symlink/root escape rejected
  - read/write/list bounds
  - diagnostics redaction
  - write serialization or safe conflict behavior
- Validation: focused plugin fs tests, relevant cargo check/test, `cargo fmt --check`, `git diff --check`, and `nix build .#yoi` because host API / packaging behavior may change.

## Non-goals

- `https` host API implementation.
- General workspace Read/Write tool delegation.
- Service / Ingress surface.
- File watcher / background sync.
- Broad WASI filesystem exposure.
- Plugin package manager / install/update.

## Related work

- `00001KV5W3PHW` — Plugin Tool execution with minimal WASM runtime.
- `00001KV5W3PJ3` — Plugin permission grant enforcement.
- `00001KVFD3YSV` — Plugin read-only CLI inspection list/show.
- `00001KSXRQ4G8` — Plugin runtime / surface / minimal host API model design.
