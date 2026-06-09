---
title: "Plugin: define extension surface for hooks and tools"
state: "planning"
created_at: "2026-05-31T01:00:05Z"
updated_at: "2026-06-03T12:25:05Z"
---

## Background

insomnia currently has internal Hook / Tool concepts, plus a separate planned MCP integration ticket (`mcp-integration`). The next design step is to define the project-level Plugin surface: how user/project-provided extensions can contribute Tools and Hooks without weakening scope, permission, history, or prompt-context invariants.

The plugin surface should not be a grab bag of arbitrary code execution. Candidate extension mechanisms have different trust and protocol properties:

- MCP: protocol-bound external tool/resource/prompt provider surface.
- TOML/config-only hooks: declarative configuration for simple hook behavior without arbitrary code.
- WASM: planned first programmable plugin runtime for Hooks and Tools, with explicit capability imports and sandboxing.
- General scripting languages: considered, but not the initial direction because arbitrary script execution broadens the trust/runtime surface too quickly.

## Related work

- `work-items/open/20260529-161928-mcp-integration/` — MCP integration as one plugin backend / external capability bridge.
- `work-items/open/20260603-122317-plugin-feature-contribution-registry/` — implementation-oriented runtime registry split-out for built-in and external feature contributions.
- `work-items/open/20260603-122317-hook-public-surface-hardening/` — prerequisite hardening for public Hook contribution safety.
- Existing internal hooks/tools code: `crates/pod`, `crates/tools`, `crates/llm-worker`.
- Manifest permission policy and scope enforcement must remain authoritative for plugin-provided tools.

## Requirements

- Define a Plugin surface that can provide:
  - Tools callable by the LLM through the normal ToolRegistry / permission / scope path;
  - Hooks observing or influencing Pod/Worker lifecycle through the existing Hook boundary, not by directly mutating worker history/context.
- Separate plugin description/registration from plugin runtime implementation.
  - A plugin manifest should declare provided tools/hooks, required capabilities, configuration schema or config values, and trust/runtime type.
  - Runtime implementations can include MCP, declarative config hooks, and WASM in separate phases.
- Keep MCP as a related backend, not the whole plugin model.
  - MCP servers remain untrusted external capability providers bridged through allowlists, bounded output, scope/permission policy, and explicit resource/prompt use.
- Define a declarative hook path for simple TOML/config-only behavior where code execution is unnecessary.
- Define a WASM plugin direction for programmable Hooks/Tools.
  - WASM modules must receive explicit host imports/capabilities only.
  - File/network/process access must not be ambient; all external effects go through host-provided capability APIs and existing policy checks.
  - Tool outputs must be bounded and recorded through normal history/tool-result paths.
- Preserve LLM context/history invariants.
  - Plugins must not inject cross-turn invisible context.
  - If plugin output becomes model-visible, it must enter through durable history/tool/hook paths according to existing rules.
- Preserve scope and permission invariants.
  - Plugin-provided tools must not bypass `ScopedFs`, manifest tool permission policy, child scope delegation, or web/network policy.
- Clarify trust model and lifecycle.
  - Builtin vs project vs user plugins.
  - Discovery/enablement through manifest/profile/config.
  - Versioning / compatibility boundaries.
  - Diagnostics when a plugin cannot load or asks for unavailable capabilities.

## Non-goals

- Implementing the full WASM runtime in the first design step.
- Implementing MCP itself beyond referencing the existing MCP integration ticket.
- Supporting arbitrary host scripting languages as a first-class plugin runtime.
- Allowing plugins to mutate session history, memory, prompt context, or scope outside approved APIs.
- Adding UI plugin systems or TUI rendering extensions.

## Suggested phases

1. **Design / architecture note**
   - Define Plugin, PluginManifest, PluginRuntimeKind, Tool contribution, Hook contribution, capability request, and trust/source model.
   - Map MCP, declarative hooks, and WASM onto that model.
2. **Internal registry boundary**
   - Detailed implementation is split to `plugin-feature-contribution-registry` so this ticket can stay focused on the architecture surface and invariants.
3. **Declarative hooks MVP**
   - Add a non-code configuration path for simple hook behavior if an immediate use case exists.
4. **WASM spike**
   - Evaluate runtime (`wasmtime` or alternative), host imports, resource limits, serialization, and Nix/package impact.
5. **MCP bridge alignment**
   - Ensure `mcp-integration` plugs into the same Tool/permission/output boundary rather than becoming a parallel extension path.

## Acceptance criteria

- The repository has a documented plugin architecture proposal covering Tools, Hooks, runtimes, capability model, trust model, and discovery/enablement.
- MCP is positioned as one plugin backend / bridge and linked to `mcp-integration`, not treated as the only extension mechanism.
- The proposal explicitly explains why arbitrary scripting languages are deferred and why WASM is the initial programmable runtime direction.
- The design preserves existing scope, permission, history, and prompt-context invariants.
- Follow-up implementation tickets can be cut independently for declarative hooks, WASM runtime, and MCP bridge integration.
- Any code changes in this ticket, if taken beyond design docs, are limited to safe internal boundaries and have focused tests.
