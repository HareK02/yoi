---
title: 'Plugin: migrate WASM Tool runtime to WebAssembly Component Model'
state: 'inprogress'
created_at: '2026-06-19T13:18:58Z'
updated_at: '2026-06-19T17:17:08Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['plugin', 'wasm', 'component-model', 'wit', 'runtime-backend', 'sandbox', 'packaging', 'sdk']
queued_by: 'workspace-panel'
queued_at: '2026-06-19T13:34:43Z'
---

## Background

Yoi's current Plugin Tool runtime uses a raw core-Wasm ABI (`yoi-plugin-wasm-1`) with `yoi_tool_call`, exported `memory`, and host imports for input/output pointer-length plumbing. That was a good MVP for a small sandboxed runtime, but it should not become the long-term authoring interface.

Common Wasm extension systems usually provide a typed SDK/PDK, manifest, capability grants, templates, and inspection tooling. The WebAssembly Component Model provides a standard way to describe typed imports/exports via WIT and canonical ABI. Adopting it early prevents `https`, `fs`, SDK, and future Service/Ingress APIs from entrenching a Yoi-specific raw ABI.

This Ticket implements an explicit Component Model runtime path for Plugin Tool packages while preserving the existing package discovery, enablement, digest pinning, ToolRegistry, ordinary Tool history, and Plugin grant enforcement boundaries.

Research and direction are persisted in:

- `.yoi/objectives/00001KVG0HR9M/item.md` — Plugin Component Model migration Objective.
- `docs/design/plugin-component-model.md` — design research and policy.
- `docs/design/plugin-packages.md` — package runtime metadata direction.

## Requirements

- Add an explicit Component Model runtime kind for Plugin packages.
  - Example manifest shape:

    ```toml
    [runtime]
    kind = "wasm-component"
    component = "plugin.component.wasm"
    world = "yoi:plugin/tool@1.0.0"
    ```

- Do not silently reinterpret existing raw core-Wasm packages.
  - Current raw runtime remains explicit, e.g. `kind = "wasm"`, `abi = "yoi-plugin-wasm-1"`.
  - Component runtime selection is driven by package manifest/runtime metadata.
- Define WIT package/worlds for the Plugin Tool runtime.
  - Tool request / response / structured error types.
  - Tool name and JSON input/output representation, or a better typed equivalent if decided during implementation.
  - Initial host API interfaces should be WIT-compatible even if some APIs remain unimplemented.
- Add a host runtime backend capable of loading and invoking Component Model Plugin Tools.
  - Evaluate whether this uses `wasmtime::component`, adapter tooling, or another backend.
  - Keep runtime selected per package; discovery/inspection must not execute Plugin code.
- Preserve existing Plugin authority boundaries.
  - Package discovery is read-only.
  - Explicit enablement is required.
  - Plugin grants are checked before Tool registration/execution and before host API calls.
  - WIT imports are not authority by themselves.
  - No ambient WASI filesystem/network/env is exposed.
- Preserve ordinary Tool behavior.
  - Component Tool registration goes through existing ToolRegistry/model-visible schema path.
  - Tool calls/results use ordinary Worker/Tool history path.
  - No hidden context injection.
- Provide at least one sample Component Model Tool Plugin.
  - Prefer Rust authoring path if feasible.
  - Plugin author source should not contain raw pointer/length ABI plumbing.
- Add or update tests for both positive and negative paths.
  - Component package discovery and manifest parsing.
  - Component Tool registration.
  - Component Tool execution.
  - Grant denial before execution / host API access.
  - Wrong world / missing export / incompatible component rejected.
  - Existing raw core-Wasm Plugin runtime either still passes or has a recorded compatibility decision.
- Measure packaging/runtime impact.
  - Binary size/build time impact if adding Wasmtime/component tooling.
  - Nix packaging changes and `cargoHash` if dependencies change.

## Acceptance criteria

- A package with `runtime.kind = "wasm-component"` and the expected WIT world can be discovered, enabled, registered as a Tool, and executed.
- A sample Component Model Tool Plugin returns a normal Tool result through the ordinary Tool path.
- Plugin author code for the sample uses generated/SDK bindings rather than raw pointer/length imports/exports.
- Component Tool execution is denied without matching Plugin grants.
- Component host imports cannot bypass Yoi's Plugin grant model.
- Unsupported / wrong WIT world or missing required export fails closed with bounded diagnostic.
- Existing raw core-Wasm runtime remains explicitly supported or a migration/deprecation decision is recorded and tests are updated accordingly.
- `yoi plugin list/show` inspection path, if available, reports Component runtime metadata without executing the component.
- Documentation is updated with authoring/runtime instructions and migration notes.
- Validation includes relevant focused tests, `cargo fmt --check`, `git diff --check`, `cargo check` / `cargo test`, and `nix build .#yoi`.

## Non-goals

- Service surface implementation.
- Ingress surface implementation.
- WebSocket / Discord Gateway bridge.
- Inbound HTTP server.
- Replacing Plugin grants with WIT imports.
- Exposing WASI filesystem/network/env as ambient authority.
- MCP integration or MCP trust policy changes.
- A full public package registry or signature/trust-chain system.

## Implementation notes

- Keep raw core-Wasm ABI compatibility separate from Component Model support.
- Prefer WIT names that can version cleanly, such as `yoi:plugin/tool@1.0.0` and `yoi:host/https@1.0.0`.
- If Wasmtime is introduced, update Nix packaging and record dependency/build-size impact.
- If a staged approach is necessary, first land WIT definitions and manifest parsing, then runtime execution in a follow-up. Do not pretend manifest parsing alone completes this Ticket.
- `https` and `fs` host API work should avoid long-term raw ABI coupling; component-compatible request/response/path/error records are preferred.

## Related work

- `00001KVG0HR9M` — Objective: Plugin Component Model migration.
- `docs/design/plugin-component-model.md` — design research and migration direction.
- `docs/design/plugin-packages.md` — package runtime metadata direction.
- `00001KV5W3PHW` — Plugin Tool execution with minimal WASM runtime.
- `00001KV5W3PJ3` — Plugin permission grant enforcement.
- `00001KVFDX9AF` — Plugin https host API.
- `00001KVFDX9AY` — Plugin fs host API.
- `00001KVFD3YSV` — Plugin read-only CLI inspection list/show.
- `00001KSXRQ4G8` — Plugin runtime / surface / minimal host API model design.
