---
title: 'MCP local stdio integration architecture'
state: 'active'
created_at: '2026-06-10T07:48:45Z'
updated_at: '2026-06-13T15:30:22Z'
---

## Objective

Add MCP local stdio integration to Yoi without weakening Worker history, prompt-context, scoped tool permission, or Plugin/Feature layering invariants.

MCP should be implemented as a protocol-backed integration layer on top of `pod::feature`. `pod::feature` supplies the contribution/lifecycle API substrate; MCP owns its own enablement, local server trust model, command/env/secret policy, and MCP-specific permissions. MCP is not the Plugin model, and Plugin permission policy is not implemented by feature-layer authority grants.

## Strategic direction

- Baseline the implementation on MCP specification `2025-11-25`.
- Start with local stdio MCP servers only.
- Treat MCP server metadata, tools, resources, prompts, and results as untrusted content.
- Do not allow MCP resources/prompts to be hidden context injection.
  - They must be explicit tool operations with history records.
- Use the normal Yoi tool registry, PreToolCall permission, history, and bounded result paths.
- Do not add private MCP-only bypasses around Worker/tool invariants.
- Keep sampling and elicitation fail-closed initially.
- Keep Streamable HTTP, remote auth, OAuth, and MCP Registry/distribution out of the first slice.
- Treat local stdio server execution as an explicit MCP config/trust decision, not as a `pod::feature` authority grant.
- Document clearly that a configured local MCP server runs as a local executable; Yoi feature authority does not sandbox its OS-level side effects.

## Layering decisions

- `pod::feature` is an API/contribution substrate.
  - It owns contribution declarations, provider/service lifecycle hooks, diagnostics, dynamic registration plumbing, and integration with normal Worker/ToolRegistry paths.
  - It does not own Plugin permission policy or MCP server trust policy.
- Plugin is a user-facing package/config/runtime layer over `pod::feature`.
  - Plugin permissions are Plugin-layer policy.
  - Plugin package discovery/enablement must not be conflated with MCP local server execution.
- MCP is a separate feature-backed integration layer.
  - MCP enablement, command/env/secret handling, server trust, and MCP-specific permission decisions live in MCP config/implementation.
  - MCP dynamic tools/resources/prompts are exposed through the feature API and ordinary Yoi tool paths.

## Work breakdown

1. `00001KTR81P9X` — Extend `pod::feature` API for protocol-backed external providers.
   - provider/service lifecycle
   - startup discovery and dynamic contribution registration
   - bounded refresh semantics
   - metadata/result normalization
   - no feature-layer authority model for MCP/Plugin permissions
2. `00001KTR82RB7` — Implement MCP `2025-11-25` local stdio server bridge.
   - explicit MCP config and trust model
   - initialize/capability negotiation
   - tools/resources/prompts list/call/read/get
   - bounded result serialization
   - list-changed diagnostics/refresh behavior
3. `00001KV0SP0TY` — Remove feature-layer HostAuthority model.
   - remove authority/grant terminology from `pod::feature`
   - keep real permission/trust policy in owning Plugin/MCP/manifest/tool layers
4. Later follow-ups, if needed.
   - richer MCP tasks / task-support integration
   - remote/HTTP transports
   - OAuth / registry / package distribution
   - Plugin package/runtime alignment, if an explicit MCP/plugin bridge is later approved

## Success criteria

- A local mock MCP server can be configured explicitly and initialized.
- Discovered MCP tools appear as ordinary Yoi tools with stable namespacing.
- Tool calls go through ordinary permission and history paths.
- MCP resources/prompts are explicit operations, not hidden context injections.
- MCP result forms are bounded and safely serialized.
- Secret values, command/env details, and server diagnostics are redacted where required.
- Local server trust boundary is documented: Yoi does not sandbox the configured executable through feature authority.
- Feature, Plugin, and MCP permission/trust responsibilities are documented as separate layers.
