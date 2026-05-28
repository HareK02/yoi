---
id: 20260528-152959-web-search-fetch-tools
slug: web-search-fetch-tools
title: Add WebSearch and WebFetch tools
status: open
kind: task
priority: P2
labels: [tools, web, llm]
created_at: 2026-05-28T15:29:59Z
updated_at: 2026-05-28T15:29:59Z
assignee: null
legacy_ticket: null
---

## Background

Insomnia currently has strong local filesystem / shell / memory tools, but the agent cannot directly consult current web information except through user-provided excerpts or shell commands. Add first-class WebSearch and WebFetch tools so the model can gather public web information through bounded, observable tool calls.

This should be implemented as normal built-in tools, not as hidden context injection. Tool calls and results must remain visible in history, subject to manifest permission policy, and bounded by output limits.

## Requirement

- Add `WebSearch` tool.
  - Input includes query string and optional result limit.
  - Output returns structured results: title, URL, snippet/summary, source/search provider metadata where available.
  - Search provider must be configurable. If no provider/API key is configured, the tool should fail with a clear diagnostic instead of falling back to scraping arbitrary search pages.
- Add `WebFetch` tool.
  - Input includes URL and optional mode/limits.
  - Output returns normalized text content plus metadata such as final URL, status, content type, title if available, and byte/token truncation indication.
  - HTML should be converted to readable text. Non-text content should be rejected or summarized only when a safe explicit handler exists.
- Add manifest configuration for web tools.
  - Enable/disable controls.
  - Search provider/API key configuration.
  - Fetch timeout, max response bytes, max output bytes/tokens, redirect limit.
  - Allowed/denied URL schemes and host policy.
- Integrate with built-in tool registration and manifest permission policy.
  - Web tools are normal tool calls and should go through the existing tool permission mechanism.
  - No implicit network access should happen outside a tool call.
- Add security and reliability protections.
  - Only `http`/`https` by default.
  - Reject local/private/link-local/loopback addresses by default unless explicitly configured.
  - Bound redirects and re-check final URLs.
  - Bound download size and output size.
  - Provide clear errors for timeout, DNS/network failure, unsupported content, blocked host/scheme, and truncation.
- Prompts/tool descriptions should tell the model when to use WebSearch vs WebFetch and that fetched content may be stale/untrusted.

## Acceptance criteria

- `WebSearch` and `WebFetch` are registered built-in tools when enabled/configured.
- Tool schemas are typed and validated.
- Manifest docs/config examples describe how to enable/configure web tools.
- Permission policy can allow/deny/ask these tools like other tools.
- Tool results are bounded and visible in history; no hidden web context is injected.
- Unit tests cover input validation, disabled/unconfigured errors, URL policy, redirect/final URL policy, output truncation, and representative HTML-to-text conversion.
- At least one integration-style test uses a local test HTTP server or mock provider rather than the public internet.
- `cargo fmt --check`
- `cargo check -p tools -p manifest -p pod`
- Relevant focused tests for tools/manifest.

## Out of scope

- Browser automation.
- Authenticated browsing / cookies / sessions.
- Javascript rendering.
- File downloads as attachments.
- Using arbitrary shell commands as the primary web access path.
- Hidden pre-request browsing or automatic web context injection.
