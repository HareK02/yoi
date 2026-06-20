---
title: 'MCP: handle list_changed notifications safely'
state: 'inprogress'
created_at: '2026-06-20T05:30:04Z'
updated_at: '2026-06-20T10:08:05Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['mcp', 'notifications', 'tool-schema', 'prompt-cache', 'refresh']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T05:59:05Z'
---

## Background

MCP servers can notify that tools/resources/prompts lists changed. Yoi must not silently go stale, but it also must not mutate the active run's model-visible tool schema or context in a way that violates history/prompt-cache invariants. This Ticket defines provider tool-list refresh behavior at safe boundaries.

## Requirements

- Handle MCP list-changed notifications: `notifications/tools/list_changed`, `notifications/resources/list_changed`, and `notifications/prompts/list_changed`.
- Choose and implement a safe refresh policy: next-turn refresh, restart/reinitialize-required diagnostic, or bounded live refresh only if it preserves schema/history invariants.
- Do not mutate current LLM context with hidden resource/prompt content.
- Emit bounded diagnostics when refresh cannot be applied safely.
- Tests with mock server notifications.

## Acceptance criteria

- list_changed does not silently stale forever.
- Current run tool schema consistency is not broken.
- Refresh/diagnostic behavior is deterministic and documented.
- Prompt-context/history invariants are preserved.
- Tests cover tools/resources/prompts list_changed and unsafe refresh fallback.

## Non-goals

- Initial tools/list registration.
- Initial resources/prompts operations.
- Remote MCP transports.

## Related work

- Depends on `00001KVHR3WS6` for tool list registration.
- Related to `00001KVHR3WSN` for resources/prompts lists.
- Objective: `00001KTR80WMN`.
