---
title: 'MCP: execute tools/call through ordinary Tool path'
state: 'inprogress'
created_at: '2026-06-20T05:30:04Z'
updated_at: '2026-06-20T09:16:53Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['mcp', 'tools-call', 'permission', 'history', 'bounded-output']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T05:59:04Z'
---

## Background

After MCP tools are registered through ToolRegistry, invoking a Yoi MCP-backed tool should call the server's `tools/call` and return a bounded ordinary Tool result. Permission denial must happen before sending a request to the MCP server.

## Requirements

- Route registered MCP tool invocation to MCP `tools/call`.
- Apply existing PreToolCall / Tool permission path before MCP server call.
- If permission is denied, do not send `tools/call` to the server.
- Distinguish normal result, MCP `isError: true`, and JSON-RPC protocol error.
- Serialize MCP result forms boundedly: `content[]`, `structuredContent`, `isError`, `_meta`, and supported rich content summaries.
- Store result through ordinary Tool result/history path.
- Treat all content as untrusted.

## Acceptance criteria

- MCP mock tool returns normal result through ordinary Yoi Tool result.
- MCP `isError: true` is represented distinctly from JSON-RPC protocol failure.
- Permission denied call is not sent to the MCP server.
- Oversize/rich results are bounded/truncated or rejected according to explicit policy.
- Tool history shows ordinary tool call/result, not hidden context injection.
- Tests cover normal result, `isError`, protocol error, permission denial, and output bounds.

## Non-goals

- resources/read or prompts/get.
- list_changed notifications.
- Sampling/elicitation.

## Related work

- Depends on `00001KVHR3WS6`.
- Objective: `00001KTR80WMN`.
