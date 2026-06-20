---
title: 'MCP: add local stdio server config and trust policy'
state: 'queued'
created_at: '2026-06-20T05:30:04Z'
updated_at: '2026-06-20T06:00:44Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['mcp', 'config', 'trust-boundary', 'secrets', 'process-exec']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T05:58:46Z'
---

## Background

MCP integration starts with explicit local stdio server configuration and trust policy. Yoi must not auto-start MCP servers from workspace presence, package discovery, or Plugin packages. A configured MCP local stdio server is a local executable running with the user's OS permissions; Yoi feature authority does not sandbox its OS-level side effects.

This Ticket only defines/parses/validates config and diagnostics. It does not spawn MCP processes or implement JSON-RPC lifecycle.

## Requirements

- Add typed Profile/config support for named local stdio MCP servers.
- Config fields must cover command, args, cwd policy, env policy, and explicit secret/env references as needed.
- No package/workspace presence auto-start.
- Validate command/env/secret config fail-closed.
- Define diagnostic surfaces for config parse/validation errors.
- Redact command/env/secret values where needed; do not write plaintext secrets to logs/model context.
- Document local executable trust boundary.
- Keep MCP config/trust separate from Plugin permissions and `pod::feature` authority.

## Acceptance criteria

- A Profile/config can declare a named local stdio MCP server.
- Invalid command/env/secret config is rejected with bounded diagnostic.
- Secrets are not emitted in plaintext diagnostics/log/model context.
- Config alone does not spawn a process.
- Docs explain that configured local MCP servers are not OS-sandboxed by Yoi feature authority.
- Tests cover valid config, invalid config, secret redaction, and no auto-start.

## Non-goals

- Spawning stdio subprocesses.
- MCP initialize/capability negotiation.
- Tool/resource/prompt registration.
- Streamable HTTP/OAuth/remote MCP.

## Related work

- Objective: `00001KTR80WMN`.
- Supersedes part of broad MCP Ticket `00001KTR82RB7`.
