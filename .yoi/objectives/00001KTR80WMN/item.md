---
title: 'MCP local stdio integration roadmap'
state: 'active'
created_at: '2026-06-10T07:48:45Z'
updated_at: '2026-06-20T05:34:00Z'
linked_tickets: ['00001KTR81P9X', '00001KV0SP0TY', '00001KVHR3WRF', '00001KVHR3WRY', '00001KVHR3WS6', '00001KVHR3WSD', '00001KVHR3WSN', '00001KVHR3WSW']
---

## Goal

Add MCP local stdio integration to Yoi without weakening Worker history, prompt-context, scoped tool permission, or Plugin/Feature layering invariants.

MCP is a protocol-backed integration layer on top of `pod::feature`. `pod::feature` supplies contribution/lifecycle/runtime-discovered registration substrate; MCP owns its own enablement, local server trust model, command/env/secret policy, and MCP-specific permission decisions. MCP is not the Plugin model, and Plugin permission policy is not implemented by feature-layer authority grants.

## Motivation / background

Yoi needs to integrate with external capability providers without turning them into hidden context sources or bypassing ordinary Tool/Worker safety rules. MCP is useful because it can expose tools, resources, and prompts from local protocol servers, but those server-provided declarations and results are untrusted and must be normalized through Yoi's existing authority boundaries.

The first MCP slice should focus on local stdio servers because they are concrete enough to implement and debug while keeping remote auth, OAuth, Streamable HTTP, registry distribution, sampling, and elicitation out of the initial trust boundary.

A configured local MCP server runs as a local executable. Yoi feature authority does not sandbox that executable's OS-level side effects, so command/env/secret handling and explicit local trust policy are MCP-layer responsibilities rather than generic `pod::feature` grants.

## Strategy / design direction

- Baseline the initial implementation on MCP specification `2025-11-25`.
- Start with local stdio MCP servers only.
- Treat MCP server metadata, tools, resources, prompts, and results as untrusted content.
- Do not allow MCP resources/prompts to become hidden context injection.
  - They must be explicit tool operations with history records.
- Use the normal Yoi ToolRegistry, PreToolCall permission, history, and bounded result paths.
- Do not add private MCP-only bypasses around Worker/tool invariants.
- Keep sampling and elicitation fail-closed initially.
- Keep Streamable HTTP, remote auth, OAuth, and MCP Registry/distribution out of the first slice.
- Treat local stdio server execution as an explicit MCP config/trust decision, not as a `pod::feature` authority grant.
- Document clearly that a configured local MCP server runs as a local executable; Yoi feature authority does not sandbox its OS-level side effects.

### Layering decisions

- `pod::feature` is an API/contribution substrate.
  - It owns contribution declarations, provider/service lifecycle hooks, diagnostics, runtime-discovered registration plumbing, and integration with normal Worker/ToolRegistry paths.
  - It does not own Plugin permission policy or MCP server trust policy.
- Plugin is a user-facing package/config/runtime layer over `pod::feature`.
  - Plugin permissions are Plugin-layer policy.
  - Plugin package discovery/enablement must not be conflated with MCP local server execution.
- MCP is a separate feature-backed integration layer.
  - MCP enablement, command/env/secret handling, server trust, and MCP-specific permission decisions live in MCP config/implementation.
  - MCP provider-discovered tools/resources/prompts are exposed through the feature API and ordinary Yoi tool paths.

### Concrete implementation tickets

Completed prerequisites:

- `00001KTR81P9X` — Extend `pod::feature` API for external protocol-backed capability providers.
- `00001KV0SP0TY` — Remove feature-layer HostAuthority model.

Concrete MCP implementation sequence:

1. `00001KVHR3WRF` — MCP local stdio server config and trust policy.
   - explicit config, command/env/secret redaction, local executable trust boundary, no auto-start.
2. `00001KVHR3WRY` — MCP stdio JSON-RPC lifecycle client.
   - subprocess lifecycle, initialize/capability negotiation, diagnostics, shutdown.
3. `00001KVHR3WS6` — MCP tools/list registration into ToolRegistry.
   - provider-discovered tools, stable namespacing, schema validation, untrusted metadata normalization, no tools/call yet.
4. `00001KVHR3WSD` — MCP tools/call execution through ordinary Tool path.
   - PreToolCall gate before server call, bounded result serialization, history path.
5. `00001KVHR3WSN` — MCP resources/prompts as explicit tool operations.
   - resources/list/read and prompts/list/get without hidden context injection.
6. `00001KVHR3WSW` — MCP list_changed notification handling.
   - deterministic safe refresh/diagnostic behavior without breaking tool schema or prompt-cache invariants.

The old broad implementation Ticket `00001KTR82RB7` is superseded by this sequence and should not be used as an implementation work item.

### Terminology

Use `runtime-discovered` or `provider-discovered` for MCP tools/resources/prompts discovered from `tools/list`, `resources/list`, or `prompts/list`. Avoid `dynamic tools` / `dynamic registry` in new MCP design prose because those phrases imply that model-visible tool schemas may change during an active LLM run.

The intended invariant is:

```text
provider-discovered at startup / provider initialization;
registered into the ordinary ToolRegistry before model exposure;
run-stable for the duration of a model request/run;
refreshed only at a safe boundary or reported as a diagnostic.
```

### Later follow-ups

- Richer MCP task/task-support integration if ordinary tool-call fallback is insufficient.
- Streamable HTTP transport.
- OAuth / remote auth.
- Registry/package distribution.
- Explicit MCP/Plugin bridge only if separately approved; do not conflate Plugin packages with MCP local server execution.

## Success criteria / exit conditions

- A local mock MCP server can be configured explicitly and initialized.
- Discovered MCP tools appear as ordinary Yoi tools with stable namespacing.
- Tool calls go through ordinary permission and history paths.
- MCP resources/prompts are explicit operations, not hidden context injections.
- MCP result forms are bounded and safely serialized.
- Secret values, command/env details, and server diagnostics are redacted where required.
- Local server trust boundary is documented: Yoi does not sandbox the configured executable through feature authority.
- Feature, Plugin, and MCP permission/trust responsibilities are documented as separate layers.

## Decision context

- MCP is not the Plugin model; it is a protocol-backed integration layer using `pod::feature` substrate.
- `pod::feature` should provide contribution/lifecycle/runtime-discovered registration plumbing, not MCP server trust policy or Plugin package permission policy.
- MCP resources and prompts must never be hidden context injection. They are explicit operations recorded through ordinary history/tool paths.
- Provider-discovered tools are discovered at startup/provider initialization and registered before model exposure; model-visible schemas remain run-stable during a request/run.
- Local stdio server execution is a user/config trust decision. Yoi does not sandbox the local executable merely because it is configured through MCP.
