---
title: 'Plugin: execute Plugin Tool with minimal WASM runtime'
state: 'inprogress'
created_at: '2026-06-15T14:48:59Z'
updated_at: '2026-06-18T12:31:01Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'wasm', 'tool-runtime', 'sandbox', 'capability-boundary', 'cancellation']
queued_by: 'workspace-panel'
queued_at: '2026-06-17T09:46:10Z'
---

## Background

Plugin Tool surface registration の後続として、enabled Plugin Tool を最小 WASM runtime で実行できるようにする。

この Ticket のゴールは、Plugin package を enable し、manifest 由来の Tool を model-visible に登録し、その Tool call を sandboxed WASM module に渡して bounded な Tool result として返す最小体験を作ること。`https` / `fs` host API、Service / Ingress、長時間 background process は扱わない。

## Requirements

- Registered Plugin Tool の invocation を Plugin runtime に route する。
- Minimal WASM runtime を追加する。
  - module load
  - tool input JSON の受け渡し
  - tool output JSON の受け取り
  - structured error handling
- Runtime は ambient authority を持たない。
  - ambient filesystem なし
  - ambient network なし
  - ambient environment variables なし
  - host API imports は明示的に許可されたものだけ
- この Ticket では host API import は最小にする。
  - tool input / output のための必要最小限のみ。
  - `https` / `fs` は実装しない。
- Bounded execution を実装する。
  - timeout
  - cancellation
  - input size bound
  - output size bound
  - error/diagnostic size bound
- Invalid Plugin output は fail closed にする。
  - malformed JSON
  - schema mismatch
  - oversize output
  - non-terminating execution
- Tool call / result は通常 Tool history 経路に乗る。
  - hidden context injection をしない。
  - Plugin stdout/stderr 相当を無制限に history に入れない。
- Runtime error は safe structured Tool error として返す。
  - panic しない。
  - secret-like host path / env / raw memory dump を出さない。
- Runtime lifecycle は Pod startup / restore と整合させる。
  - package digest / runtime config に基づいて deterministic に module を選ぶ。
  - runtime-only mutable state に依存して Tool availability を決めない。

## Acceptance criteria

- Sample Plugin package の Tool を WASM runtime 経由で実行できる。
- Tool input JSON が WASM module に渡り、Tool output JSON が通常 Tool result として返る。
- Tool result は通常の history / permission / trace 経路に残る。
- Plugin Tool が ambient filesystem / network / env にアクセスできない。
- Timeout する Plugin execution は中断され、安全な Tool error になる。
- Oversize output / malformed output / schema mismatch は fail closed する。
- Cancellation が Worker / Tool execution の cancellation と整合する。
- Runtime diagnostics は bounded で、selected secret/path/env を漏らさない。
- Tests cover:
  - successful WASM Plugin Tool execution
  - malformed output rejected
  - oversize output rejected
  - timeout / cancellation
  - missing runtime module diagnostic
  - no ambient fs/network/env by default
  - tool result history path remains ordinary tool result
- Validation: focused plugin runtime tests, `cargo fmt --check`, relevant `cargo check` / `cargo test`, `git diff --check`, and `nix build .#yoi` if dependencies / packaging change.

## Non-goals

- `https` host API.
- `fs` host API.
- Service surface.
- Ingress surface.
- Long-running Plugin daemon/process lifecycle.
- Plugin package manager / registry.
- Signature/trust-chain verification.
- Broad WASI surface beyond the explicitly needed imports.

## Related work

- `00001KV5R5V2S` — Plugin package discovery and explicit enablement resolver.
- `00001KV5W3PHA` — Plugin Tool surface registration.
- `00001KV5W3PJ3` — Plugin permission grant enforcement.
- `00001KSXRQ4G8` — Plugin runtime / surface / host API model design.
