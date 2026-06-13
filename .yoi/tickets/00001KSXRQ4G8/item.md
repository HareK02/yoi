---
title: 'Plugin: define extension surface for hooks and tools'
state: 'planning'
created_at: '2026-05-31T01:00:05Z'
updated_at: '2026-06-13T15:29:21Z'
assignee: null
---

## Goal

Design Yoi's Plugin surface as the user-facing package/config/runtime layer built on top of `pod::feature`.

`pod::feature` is the internal API substrate for contribution registration: Tools, Hooks, BackgroundTasks, Services, diagnostics, and lifecycle integration. Plugin is the layer that makes that substrate usable by users and workspaces through package metadata, enablement config, runtime choices, provenance, and Plugin-layer permissions.

MCP is not the Plugin model and should not be treated as a Plugin permission backend. MCP is a separate protocol-backed integration layer that also uses the feature API and owns its own MCP-specific enablement/trust policy.

## Background

Yoi already has internal extension-like surfaces:

- Tools via `llm_worker::ToolRegistry` / `ToolDefinition`
- Hooks via `HookRegistry` / `HookEvent` / `HookAction`
- Feature contribution declarations in `pod::feature`
- Built-in features such as task and ticket tools

The Plugin surface should expose extension points without letting external packages bypass Worker history, prompt/context invariants, scoped tool permissions, or role/profile configuration.

Prior Hook hardening work constrains model-visible context mutation so Plugin exposure can stay safe. Plugin design should reuse those safe surfaces rather than creating a parallel tool/hook runtime.

## Requirements

- Define Plugin as a user-facing layer over `pod::feature`, not as the feature API itself.
  - Plugin package/config/runtime code eventually contributes through `pod::feature`.
  - `pod::feature` remains responsible for contribution declarations, lifecycle, diagnostics, and registration plumbing.
  - Plugin remains responsible for package enablement, provenance, runtime selection, and Plugin-layer permission/trust policy.
- Define supported contribution categories.
  - Tools
  - Hooks
  - Background tasks / services, if included in the initial model
  - Future extensions must remain explicit contribution categories, not arbitrary host access.
- Define Plugin-layer permission and trust model separately from `pod::feature`.
  - Do not rely on feature-level `HostAuthority` / authority grants for Plugin permissions.
  - Plugin grants should be tied to package identity/runtime/user or workspace config, not to feature install reports.
  - Plugin policy must decide which contribution categories and host APIs a Plugin runtime can use.
- Define runtime families separately.
  - Declarative/config-only Plugins
  - WASM or another sandboxed runtime, if/when selected
  - External process/plugin bridge, if/when selected
  - MCP remains a separate feature-backed protocol integration, not the Plugin permission model.
- Hook Plugins must use the hardened public Hook surface.
  - no raw hidden `Item` injection
  - no prompt/context mutation outside durable history-aware paths
  - explicit supported `HookAction` subset only
- Tool Plugins must register ordinary Yoi tools.
  - normal tool schema exposure
  - normal PreToolCall permission path
  - normal tool-result history path
  - bounded result serialization
- Plugin metadata and package content are untrusted input.
- Plugin enablement must be explicit.
  - package presence/discovery alone cannot enable or execute Plugin code
  - workspace/user config must opt in to package/runtime/contribution activation
- The design must document the boundary among Plugin, MCP, and built-in Features.

## Acceptance criteria

- A design note or Ticket plan defines Plugin as a user-facing layer over `pod::feature`.
- The plan states that Plugin permissions are Plugin-layer policy and are not implemented by `pod::feature` `HostAuthority` grants.
- The plan states that MCP is a separate feature-backed integration with MCP-specific enablement/trust policy.
- Contribution categories and runtime families are named with explicit in/out of scope decisions for the first implementation slice.
- The public Hook safety invariants from `00001KT6Q08R8` remain intact.
- Tool Plugins are required to use ordinary ToolRegistry / permission / history / bounded-result paths.
- No design path allows Plugin package presence alone to execute code or mutate context.
- The design identifies which follow-up Tickets own package format, runtime implementation, and Plugin permission details.

## Suggested decomposition

1. Public Hook surface hardening. Completed by `00001KT6Q08R8`.
2. Feature contribution API substrate. Completed initial slice by `00001KT6Q08R9`; dynamic provider work continues in `00001KTR81P9X`.
3. Plugin package/discovery format. Tracked by `00001KT0Z4BK8`.
4. Plugin permission/runtime policy Ticket. Create or refine after runtime family is selected; do not put this policy into `pod::feature`.
5. MCP local stdio integration. Tracked separately by `00001KTR82RB7`; it uses feature API but is not Plugin permission work.

## Notes

- This Ticket is a design coordination Ticket; implementation should be split into concrete Tickets rather than landing a broad Plugin platform in one change.
- Keep Plugin permission terms distinct from `HostAuthority`. If `HostAuthority` remains in code, it is not the Plugin policy surface.
- Avoid adding support for arbitrary scripting languages until package identity, runtime sandbox/trust, and permission policy are explicit.
