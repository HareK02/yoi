---
title: "Plugin Component Model migration"
state: "active"
created_at: "2026-06-19T13:18:58Z"
updated_at: "2026-06-19T13:18:58Z"
linked_tickets: ["00001KV5W3PHW", "00001KVFDX9AF", "00001KVFDX9AY", "00001KVFD3YSV", "00001KVG0HR96"]
---

## Goal

Move Yoi Plugin Tool authoring and runtime from the current raw core-Wasm ABI toward the WebAssembly Component Model so Plugin authors can implement typed interfaces rather than hand-writing pointer/length memory plumbing.

The target is not to weaken sandboxing or Plugin grants. The target is to make Plugin ABI, host APIs, SDK bindings, and future multi-language authoring use typed WIT worlds while preserving Yoi's existing package discovery, explicit enablement, digest pinning, ToolRegistry, ordinary Tool history, and Plugin-layer permission enforcement.

## Motivation / background

Yoi currently has a functional minimal Plugin Tool MVP:

- package discovery / explicit enablement;
- Tool surface registration;
- minimal WASM Tool execution through a raw core-Wasm ABI;
- Plugin permission grants.

The raw ABI is intentionally narrow, but it pushes too much low-level work onto Plugin authors and future SDKs:

- `memory` export handling;
- `yoi_tool_call()` export convention;
- `input_len` / `input_read` / `output_write` pointer-length plumbing;
- JSON serialization and error encoding;
- host API import namespace conventions;
- ABI versioning for future `https` / `fs` / Ingress APIs.

Research of common Wasm extension systems points in the same direction:

- Extism-style systems provide a PDK so Plugin authors write normal typed functions while the PDK hides the raw ABI.
- Spin-style systems combine manifests, templates, SDKs, explicit outbound/file capabilities, and Wasm components.
- The WebAssembly Component Model standardizes typed imports/exports via WIT and canonical ABI instead of each host inventing its own pointer/length protocol.

Yoi should adopt that direction early enough that `https`, `fs`, SDK, authoring templates, and future Service/Ingress surfaces do not entrench a custom ABI that will be expensive to migrate later.

## Strategy / design direction

- Treat the Component Model as the preferred future Plugin runtime shape.
  - New typed Plugin host APIs should be designed in WIT-compatible terms.
  - `runtime.kind = "wasm-component"` should become the preferred runtime once implemented.
- Keep the current raw core-Wasm runtime as a compatibility / migration bridge until Component Model execution is validated.
  - Current ABI name: `yoi-plugin-wasm-1`.
  - Future component world should get an explicit version, e.g. `yoi:plugin/tool@1.0.0`.
- Preserve existing Yoi authority boundaries.
  - Component imports are capability-shaped interfaces, not grants by themselves.
  - Package discovery remains read-only.
  - Enablement and Plugin grants remain explicit and fail closed before registration/execution/host API calls.
  - Tool calls and results continue through ordinary ToolRegistry / history paths.
- Prefer a phased migration rather than a flag day.
  1. Define WIT package/worlds for Tool Plugin and host APIs.
  2. Add runtime manifest support for `wasm-component` alongside current `wasm`/core ABI.
  3. Add host-side component execution and typed import/export binding.
  4. Port or wrap `https` / `fs` host APIs through WIT-compatible interfaces.
  5. Add authoring SDK/templates around the component world.
  6. Keep or deprecate raw core-Wasm runtime based on migration evidence.
- Evaluate runtime cost explicitly.
  - Current implementation uses `wasmi` for core Wasm.
  - Component Model support likely points to `wasmtime::component` or a separate component backend, with packaging/build-size/runtime implications.
  - Nix build validation is required for this migration.

## Success criteria / exit conditions

- Yoi has a documented WIT world for Tool Plugins and initial host APIs.
- A sample Rust Component Model Tool Plugin can be built and run by Yoi without raw pointer/length code in Plugin author source.
- Component runtime is selected explicitly by package manifest metadata and never by package presence alone.
- Component host API calls are still denied without matching Plugin grants.
- Tool outputs still use ordinary Tool result/history paths; no hidden context injection is introduced.
- Current core-Wasm Plugin tests either remain passing or are explicitly migrated with a compatibility decision.
- Packaging impact is measured and `nix build .#yoi` passes.
- Documentation explains the migration path for existing raw-ABI Plugin packages and future SDK authors.

## Decision context

- This Objective is medium-term context, not Ticket authority. Implementation still requires reading concrete Ticket bodies, threads, artifacts, and relations.
- The Component Model direction supersedes making Yoi's custom raw ABI the long-term authoring interface.
- Short-term `https` / `fs` work should avoid encoding choices that conflict with later WIT typing.
- Guest SDK work should either target Component Model directly or keep the raw ABI wrapper clearly transitional.
- MCP remains separate from Plugin. Component Model adoption for Plugin does not imply MCP server execution, MCP prompt/resource injection, or MCP trust policy changes.
