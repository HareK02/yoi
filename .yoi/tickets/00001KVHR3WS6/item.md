---
title: 'MCP: register server tools into ToolRegistry'
state: 'inprogress'
created_at: '2026-06-20T05:30:04Z'
updated_at: '2026-06-20T08:00:53Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['mcp', 'tools-list', 'tool-registry', 'schema', 'untrusted-metadata']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T05:58:58Z'
---

## Background

Once a configured MCP stdio server can initialize, Yoi should expose discovered MCP tools as ordinary model-visible Yoi tools through the existing ToolRegistry path. Server-provided tool metadata and schemas are untrusted data.

This Ticket only registers tools discovered through `tools/list`. It does not implement `tools/call` execution.

## Requirements

- Call MCP `tools/list` after initialize where supported.
- Handle pagination / bounded listing.
- Normalize MCP tool names into stable namespaced Yoi tool names that include server namespace.
- Validate/normalize tool descriptions and input schemas as untrusted metadata.
- Reject invalid schemas, duplicate names, and collisions fail-closed with diagnostics.
- Register contributions through `pod::feature` / normal ToolRegistry path; no private MCP bypass.
- Do not register resources/prompts in this Ticket.

## Acceptance criteria

- MCP mock server tool appears as model-visible Yoi tool with stable namespaced name.
- Invalid schema is rejected with bounded diagnostic.
- Duplicate/colliding names are rejected fail-closed.
- Server metadata cannot weaken Yoi instructions/scope/permissions.
- No `tools/call` request is sent during registration.
- Tests cover valid registration, pagination/bounds, invalid schema, duplicate/collision, and untrusted metadata normalization.

## Non-goals

- MCP tool execution.
- Resources/prompts operations.
- list_changed notifications.

## Related work

- Depends on `00001KVHR3WRY`.
- Objective: `00001KTR80WMN`.
