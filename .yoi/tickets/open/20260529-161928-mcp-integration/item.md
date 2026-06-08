---
id: 20260529-161928-mcp-integration
slug: mcp-integration
title: MCP integration as external tool/resource/prompt provider
status: open
kind: feature
priority: P2
labels: [mcp, tools, security, profiles]
workflow_state: planning
created_at: 2026-05-29T16:19:28Z
updated_at: 2026-05-29T16:19:28Z
assignee: null
legacy_ticket: null
---

## Background

MCP (Model Context Protocol) is an open JSON-RPC based protocol for connecting AI applications to external systems. MCP servers can expose tools, resources, and prompts; clients/hosts can also expose capabilities such as roots, elicitation, sampling, logging, progress, and cancellation. Common transports are local stdio and remote Streamable HTTP.

Insomnia already has a built-in tool registry, manifest/profile-driven policy, scoped filesystem permissions, prompt/workflow assets, and bounded tool output. MCP should integrate with those existing safety and orchestration layers rather than bypass them.

MCP servers must be treated as untrusted external capability providers. Tool descriptions, annotations, resource content, and prompt templates returned by a server are data from an external system and must not implicitly weaken Insomnia scope, permission, prompt-context, or history-persistence rules.

## Requirements

- Add MCP server configuration through the manifest/profile system.
  - Profiles should be able to declare named MCP servers.
  - Server configuration should support local stdio as the first transport.
  - Streamable HTTP can be a later phase, but the design should not preclude it.
  - Server process commands, arguments, environment/credential references, and working directory must be explicit.
- Implement MCP client foundation.
  - JSON-RPC 2.0 message handling.
  - lifecycle initialization and capability negotiation.
  - `notifications/initialized` after successful initialization.
  - graceful shutdown and clear diagnostics for startup/protocol failures.
- Bridge MCP tools into Insomnia's tool system.
  - Use `tools/list` to discover tools.
  - Register discovered MCP tools under stable names that include the server namespace.
  - Execute tool calls through `tools/call`.
  - Preserve existing PreToolCall permission policy and manifest/profile tool controls.
  - Bound tool result size and serialize results without allowing server-provided data to act as instructions.
  - Support `notifications/tools/list_changed` by refreshing the registered tool list when safe.
- Treat MCP resources and prompts as explicit context sources, not hidden injections.
  - Initial implementation may defer resources/prompts, but the design must specify that `resources/read` and `prompts/get` are explicit user/model-visible operations with permission/policy gates.
  - Do not silently inject resource or prompt content into LLM context outside history.
- Connect filesystem roots to Insomnia scope.
  - If MCP roots are supported, expose only authorized scope roots.
  - A server must not learn or operate on paths outside the configured scope.
- Keep client-side MCP capabilities conservative.
  - Sampling is powerful and should be disabled initially or require explicit approval because it lets an MCP server request LLM completions.
  - Elicitation should require an approval/UI path before a server can request user input.
  - Logging/progress notifications should be surfaced as diagnostics without polluting model context.
- Security and trust constraints.
  - Tool descriptions and schemas from MCP servers are untrusted metadata.
  - All MCP tool invocations remain subject to Insomnia tool permission policy.
  - Server-provided content must not override system/developer instructions.
  - Secrets are passed only through explicit env/secret references and must not be logged or exposed to model context.
  - Remote MCP servers require an explicit future design for auth, TLS, redirects, private network policy, and output limits.
- UX and observability.
  - Startup failures should identify the MCP server and failing phase.
  - Tool list changes and server disconnects should be visible to the user/TUI.
  - Provide enough diagnostics for debugging without printing secrets.
- Documentation.
  - Explain MCP's trust model in Insomnia.
  - Show examples for local stdio MCP servers.
  - Document how MCP tool names, permissions, scope, and profiles interact.

## Suggested implementation phases

1. stdio MCP client foundation with mock-server tests.
2. `tools/list` / `tools/call` bridge into the existing tool registry and permission policy.
3. Manifest/profile configuration and CLI/Pod startup integration.
4. TUI diagnostics for server startup/disconnect/tool-list changes.
5. Resources/prompts support as explicit operations.
6. Streamable HTTP transport and auth design.
7. Sampling/elicitation only after an approval/resume protocol exists.

## Acceptance criteria

- A manifest/profile can configure at least one local stdio MCP server.
- Pod startup initializes the configured MCP server and reports clear diagnostics on failure.
- `tools/list` results are registered as Insomnia tools with namespaced stable names.
- Calling a registered MCP tool invokes `tools/call` and returns bounded structured output to the model.
- Existing tool permission policy applies to MCP tools before execution.
- MCP server metadata/content is treated as untrusted and does not bypass prompt-context or history-persistence rules.
- Secrets used by MCP server configuration are represented as explicit env/secret references and are not logged or serialized as plaintext.
- Filesystem roots, if exposed, are derived from authorized Insomnia scope only.
- Sampling and elicitation are disabled or fail closed unless an explicit approval path is implemented.
- Focused tests cover protocol lifecycle, tool discovery, tool call success/failure, permission denial, bounded output, server disconnect, and no-secret diagnostics.
- Documentation includes a local stdio MCP server example and security guidance.
- `cargo fmt --check`
- Relevant manifest/tools/pod/tui tests pass.
