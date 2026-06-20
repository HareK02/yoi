---
title: 'MCP: expose resources and prompts as explicit tool operations'
state: 'inprogress'
created_at: '2026-06-20T05:30:04Z'
updated_at: '2026-06-20T09:38:10Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['mcp', 'resources', 'prompts', 'prompt-context', 'history', 'untrusted-content']
queued_by: 'workspace-panel'
queued_at: '2026-06-20T05:58:57Z'
---

## Background

MCP resources and prompts must not become hidden context injection. They should be exposed as explicit Yoi tool operations whose results are recorded through ordinary Tool result/history paths.

## Requirements

- Expose MCP resources/prompts as explicit namespaced Yoi tool operations: `resources/list`, `resources/read`, `prompts/list`, and `prompts/get`.
- Treat returned content/templates as untrusted tool result data.
- Do not inject resource/prompt content directly into context outside history/tool result.
- Bound result sizes and rich/embedded content serialization.
- Handle pagination/list bounds where applicable.
- Diagnostics identify server/resource/prompt operation without leaking secrets.

## Acceptance criteria

- `resources/list` and `resources/read` can be invoked as explicit tools.
- `prompts/list` and `prompts/get` can be invoked as explicit tools.
- Results are ordinary Tool results and history records.
- No hidden context injection path is introduced.
- Oversize/rich content is bounded.
- Tests cover list/read/get happy paths, untrusted content, bounds, and no hidden injection.

## Non-goals

- MCP tool execution itself.
- list_changed notification refresh.
- Sampling/elicitation.

## Related work

- Depends on `00001KVHR3WRY`.
- Related to `00001KVHR3WSD` for result serialization policy.
- Objective: `00001KTR80WMN`.
