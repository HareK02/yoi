---
title: "Plugin platform roadmap"
state: "active"
created_at: "2026-06-19T13:18:58Z"
updated_at: "2026-06-24T19:55:00Z"
linked_tickets: ["00001KV5R5V2S", "00001KV5W3PHA", "00001KV5W3PHW", "00001KV5W3PJ3", "00001KVFD3YSV", "00001KVFDX9AF", "00001KVFDX9AY", "00001KVG0HR96", "00001KVXHVCR5", "00001KVXK0WD3", "00001KVXK0WDH", "00001KVXK0WDQ", "00001KVXK0WDX", "00001KVXK0WE4", "00001KVXK0WEA"]
---

## Goal

Build Yoi's Plugin platform as a coherent extension system: packages are discovered and inspected safely, enabled explicitly, registered through typed Plugin surfaces, executed in a sandboxed runtime, constrained by Plugin-layer grants, and authored through SDK/templates rather than raw runtime ABI details.

The long-term platform goal is not merely to run Wasm. It is to make Plugin packages a durable, inspectable, permissioned, and authorable extension layer for Tools first, then host APIs (`https`, `fs`), and later Service / Ingress surfaces when concrete needs justify them.

## Motivation / background

The current Plugin foundation is already substantial:

- package discovery and explicit enablement resolver;
- Tool surface registration through the ordinary ToolRegistry/model-visible schema path;
- minimal sandboxed WASM Tool execution;
- Plugin permission grant enforcement;
- follow-up Tickets for read-only inspection CLI, `https`, `fs`, and Component Model migration.

The remaining work must be kept as one roadmap because the pieces constrain each other:

- Plugin authoring needs an SDK/PDK and examples, not raw pointer/length Wasm ABI hand-coding.
- `https` and `fs` host APIs must be grant-gated and shaped so they can move cleanly to typed Component Model interfaces.
- Diagnostics (`yoi plugin list/show`) are needed before the system becomes harder to debug.
- Component Model adoption should guide new host API design before a custom raw ABI becomes entrenched.
- Service / Ingress are useful for bridge-style integrations, but should come after Tool runtime, diagnostics, and host API policy are stable.

Research of common Wasm extension systems points to the same pattern: mature systems combine a package manifest, explicit capabilities, a sandbox runtime, host-provided capability APIs, language SDK/PDK bindings, templates/examples, inspection/check tooling, and versioned interfaces.

## Strategy / design direction

- Keep Plugin as a user-facing package/config/runtime layer above lower-level `pod::feature` substrate.
  - `pod::feature` provides contribution/registration substrate.
  - Plugin owns package discovery, enablement, grant policy, runtime selection, authoring UX, and user-facing diagnostics.
- Preserve authority boundaries.
  - Package discovery is read-only inventory.
  - Package presence never registers a Tool/Hook, executes Wasm, starts a Service, reads files, opens network, or injects context.
  - Explicit enablement and Plugin grants are required before registration/execution/host API use.
  - Tool calls/results continue through ordinary ToolRegistry and Worker history paths.
- Treat Component Model as the active Plugin runtime shape before public release.
  - New typed Plugin host APIs should be designed in WIT-compatible terms.
  - `runtime.kind = "wasm-component"` is the current Plugin runtime authority for new work.
  - The earlier `yoi-plugin-wasm-1` raw core-Wasm compatibility bridge is retired from the active roadmap because Plugin has not been publicly released and compatibility would preserve the wrong boundary.
- Sequence the platform in usable layers:
  1. Package discovery / explicit enablement / digest-pinned restore. Completed foundation.
  2. Tool surface registration. Completed foundation.
  3. Minimal WASM Tool execution. Completed foundation.
  4. Permission grants. Completed foundation.
  5. Read-only Plugin CLI inspection (`yoi plugin list/show`) for debugging discovery/enablement/grants/runtime metadata.
  6. `https` and `fs` host APIs for Tool Plugins, grant-gated and WIT-compatible.
  7. Remove the raw core-Wasm compatibility bridge and reject legacy runtime manifests.
  8. Component Model runtime and authoring model become the only active Plugin runtime path.
  9. Guest SDK/PDK, examples, `check`/`pack`/`new` authoring tooling target Component Model only.
  10. Service / Ingress runtime is developed as host-managed lifecycle, event queue, output command, and diagnostics slices.
  11. WebSocket support for long-lived integrations is host-owned connection driver + ingress event delivery + output command, not Plugin-owned polling with `recv(timeout)`.
- Keep Discord-style bridge goals split into two stages.
  - Outbound Discord/webhook Tool is possible after `https`.
  - Bidirectional Discord bridge requires Service + Ingress + WebSocket or inbound HTTP and host routing policy.

## Current implementation split

The broad Plugin runtime redesign is tracked by `00001KVXHVCR5` as context only; implementation should proceed through concrete Tickets instead of routing that umbrella as a single coding task.

1. `00001KVXK0WD3` Remove legacy raw WASM Plugin runtime.
   - Deletes the active `LegacyToolAdapter` / raw-Wasm execution path.
2. `00001KVXK0WDH` Reject legacy Plugin runtime in manifest and CLI diagnostics.
   - Makes `plugin.toml`, `yoi plugin check/list/show`, docs, and fixtures reflect Component Model only runtime authority.
3. `00001KVXK0WDQ` Define Plugin Service lifecycle and ingress queue runtime.
   - Adds host-managed lifecycle, bounded queue, serial dispatch, backpressure, timeout, and diagnostics.
4. `00001KVXK0WDX` Add Plugin service output command model.
   - Lets service handlers request side effects as grant-checked commands rather than ambient authority.
5. `00001KVXK0WE4` Add host-owned WebSocket driver for Plugin services.
   - Converts incoming WS frames into ingress events and sends outbound frames through output commands.
6. `00001KVXK0WEA` Update Plugin WIT PDK templates for service event runtime.
   - Aligns authoring API, WIT, PDK, templates, and docs with the new event/command execution model.

## Success criteria / exit conditions

- Users can inspect Plugin discovery/enablement/grant/runtime state through a read-only CLI without executing Plugin code.
- Plugin authors can build a Tool Plugin without writing raw memory/pointer ABI plumbing.
- Tool Plugins can safely call grant-gated `https` and `fs` host APIs.
- Component Model is the only active Plugin runtime path, with WIT-compatible host API types and measured packaging/runtime impact.
- Plugin grants remain authoritative over registration, execution, and host API calls.
- Plugin diagnostics explain missing package, invalid manifest, digest/version mismatch, missing grant, rejected schema, runtime mismatch, legacy runtime rejection, and unsupported host API cases safely.
- Raw core-Wasm Plugin compatibility is removed before public release; tests and docs no longer treat it as a current runtime.
- Documentation covers package format, Component Model runtime, host API authority, authoring SDK/templates, Service/Ingress event runtime, and operational debugging.
- Service/Ingress work is host-managed: services have lifecycle/status, ingress uses bounded queues, side effects are output commands, and WebSocket integrations use host-owned connection drivers.

## Decision context

- This Objective is roadmap context, not Ticket authority. Implementation still requires reading concrete Ticket bodies, threads, artifacts, and relations.
- Component Model direction supersedes Yoi's custom raw ABI as both long-term and current active Plugin runtime authority before public release.
- `https` / `fs` work should avoid choices that conflict with later WIT typed interfaces.
- Guest SDK work targets Component Model directly; raw ABI wrappers are not a supported transitional authoring path.
- Plugin and MCP remain separate. Component Model adoption for Plugin does not imply MCP server execution, MCP prompt/resource injection, or MCP trust policy changes.
- Plugin surfaces remain Tool / Hook / Service / Ingress; outbound side effects are Tool metadata and host API grants, not a separate surface.
