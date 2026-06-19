---
title: 'Plugin: implement https host API for Tool runtime'
state: 'inprogress'
created_at: '2026-06-19T07:53:13Z'
updated_at: '2026-06-19T15:32:15Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'https', 'host-api', 'network', 'sandbox', 'secrets', 'permission-grants']
queued_by: 'workspace-panel'
queued_at: '2026-06-19T10:19:53Z'
---

## Background

Plugin Tool runtime は minimal WASM execution と permission grants まで実装済みだが、外部 HTTPS API を呼ぶ host API はまだ未実装である。

この Ticket では、WASM Plugin Tool から明示 grant された outbound HTTPS request だけを実行できる `https` host API を追加する。これは Discord webhook / REST API など outbound integration の前提になる。ただし Service / Ingress / WebSocket / inbound HTTP はこの Ticket の対象外。

用語は `web` ではなく `https` とする。

## Requirements

- WASM Plugin Tool runtime に `https` host API import を追加する。
  - API 名・ABI は既存 `yoi-plugin-wasm-1` / host import 設計と整合させる。
  - Plugin は ambient network access を持たず、host API 経由のみで HTTPS request できる。
- HTTPS only とする。
  - `http://` は reject。
  - localhost / private / link-local / unix socket / file URL 等は reject。
- Permission grants と統合する。
  - manifest requested permissions の `host_api.https` を読む。
  - config granted permissions と照合する。
  - grant がない場合は fail closed。
  - host / method / optional path prefix などの allowlist を表現できるようにする。
- Request を bounded にする。
  - method allowlist。
  - request body size bound。
  - header count / size bound。
  - response body size bound。
  - timeout。
  - redirect policy。
- Credentials は ambient env から読まない。
  - header / auth は explicit config / secret ref 経由だけにする。
  - diagnostics に secret-like header / token / body content を漏らさない。
- Response は Tool result に安全に戻せる bounded structure にする。
  - status code
  - bounded headers if needed
  - bounded body text / bytes policy
  - truncated flag
- Failure は structured Tool error にする。
  - grant denied
  - URL rejected
  - private/local host rejected
  - timeout
  - response too large
  - network error
  - unsupported method
- Plugin code / history / model context に hidden context injection しない。
  - HTTPS response は Tool result として通常の tool history 経路に残す。

## Acceptance criteria

- Granted Plugin Tool can perform an allowed HTTPS request through host API.
- Request without `host_api.https` grant fails closed before network access.
- Disallowed host / method / URL scheme fails closed.
- `http://`, localhost, private IP, link-local, and local/private host targets are rejected.
- Timeout and response size bounds are enforced.
- Request / response diagnostics are bounded and redact secret-like values.
- No ambient env credentials or ambient network APIs are exposed to WASM.
- Tool result path remains ordinary Tool result/history path.
- Tests cover:
  - allowed HTTPS request with grant
  - missing grant denied
  - disallowed host denied
  - method denied
  - http scheme denied
  - private/local host denied
  - timeout
  - response truncation / size bound
  - secret header redaction
  - no network access without host API import/grant
- Validation: focused plugin https tests, relevant cargo check/test, `cargo fmt --check`, `git diff --check`, and `nix build .#yoi` because dependency/package/network code may change.

## Non-goals

- `fs` host API implementation.
- WebSocket / SSE / timer host APIs.
- Service surface lifecycle.
- Ingress surface.
- Discord Gateway bridge.
- Inbound HTTP server.
- Plugin package manager / install/update.

## Related work

- `00001KV5W3PHW` — Plugin Tool execution with minimal WASM runtime.
- `00001KV5W3PJ3` — Plugin permission grant enforcement.
- `00001KVFD3YSV` — Plugin read-only CLI inspection list/show.
- `00001KSXRQ4G8` — Plugin runtime / surface / minimal host API model design.
