---
title: 'MCP: register server tools into ToolRegistry'
state: 'queued'
created_at: '2026-06-20T05:30:04Z'
updated_at: '2026-06-20T05:58:58Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['mcp', 'tools-list', 'tool-registry', 'schema', 'untrusted-metadata']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T05:58:58Z'
---

## Background

Once a configured MCP stdio server can initialize, Yoi should expose provider-discovered MCP tools as ordinary model-visible Yoi tools through the existing ToolRegistry path. Server-provided tool metadata and schemas are untrusted data.

This Ticket only registers tools discovered through `tools/list` at provider initialization / safe refresh boundaries. It does not implement `tools/call` execution and does not allow model-visible tool schema mutation during an active run.

## Requirements

- Call MCP `tools/list` after initialize where supported.
- Handle pagination / bounded listing.
- Normalize MCP tool names into stable namespaced Yoi tool names that include server namespace.
- Validate/normalize tool descriptions and input schemas as untrusted metadata.
- Reject invalid schemas, duplicate names, and collisions fail-closed with diagnostics.
- Register provider-discovered tool contributions through `pod::feature` / normal ToolRegistry path; no private MCP bypass.
- Keep model-visible tool schema run-stable; `list_changed` handling is a later safe-boundary refresh/diagnostic problem, not mid-run mutation.
- Do not register resources/prompts in this Ticket.

## Acceptance criteria

- Provider-discovered MCP mock server tool appears as model-visible Yoi tool with stable namespaced name.
- Invalid schema is rejected with bounded diagnostic.
- Duplicate/colliding names are rejected fail-closed.
- Server metadata cannot weaken Yoi instructions/scope/permissions.
- No `tools/call` request is sent during registration.
- Active-run model-visible schema is not mutated by this registration path.
- Tests cover valid registration, pagination/bounds, invalid schema, duplicate/collision, untrusted metadata normalization, and run-stable schema behavior.

## Non-goals

- MCP tool execution.
- Resources/prompts operations.
- list_changed notifications.

## Related work

- Depends on `00001KVHR3WRY`.
- Objective: `00001KTR80WMN`.
