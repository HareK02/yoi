---
title: 'MCP: implement stdio JSON-RPC lifecycle client'
state: 'done'
created_at: '2026-06-20T05:30:04Z'
updated_at: '2026-06-20T07:59:10Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['mcp', 'stdio', 'json-rpc', 'process-lifecycle', 'diagnostics']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T05:58:54Z'
---

## Background

After MCP local stdio server config exists, Yoi needs a lifecycle client that can start a configured server, speak newline-delimited JSON-RPC over stdio, perform MCP initialize/capability negotiation, and shut down safely.

This Ticket creates the protocol/lifecycle foundation only. It does not expose MCP tools/resources/prompts to the model-visible ToolRegistry.

## Requirements

- Spawn configured local stdio MCP servers from explicit config only.
- Use stdin/stdout newline-delimited JSON-RPC.
- Treat stdout as protocol messages.
- Treat stderr as bounded diagnostics/logging, not automatic protocol failure.
- Implement initialize, capability negotiation, and notifications/initialized.
- Track server name and startup phase in diagnostics.
- Implement graceful shutdown, terminate, and kill fallback.
- Handle process exit/disconnect/startup failure with bounded diagnostics.
- Do not declare sampling or elicitation client capabilities initially.
- Add local mock MCP server tests.

## Acceptance criteria

- Mock local stdio MCP server initializes successfully.
- Initialize failure reports server name and phase.
- stderr is bounded and redacted where needed.
- Shutdown is safe and deterministic.
- Sampling/elicitation are not advertised and fail closed if requested.
- No tools/resources/prompts are registered by this Ticket.

## Non-goals

- tools/list ToolRegistry registration.
- tools/call execution.
- resources/prompts operations.
- Streamable HTTP/OAuth/remote MCP.

## Related work

- Depends on `00001KVHR3WRF`.
- Objective: `00001KTR80WMN`.
