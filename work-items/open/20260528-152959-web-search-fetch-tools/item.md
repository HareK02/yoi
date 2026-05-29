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

## Brave Search API notes

`https://github.com/brave/brave-search-mcp-server` was checked as the reference implementation. It is an MCP server around Brave Search APIs, not a general page fetcher.

Useful details to mirror for `WebSearch`:

- Use `https://api.search.brave.com/res/v1/web/search` for the first provider.
- Authenticate with `X-Subscription-Token: <api key>`; Insomnia should read the key from a configured environment variable such as `BRAVE_SEARCH_API_KEY` rather than storing raw secrets in the manifest.
- Send `q` for the query. Validate query length up front: Brave's MCP server caps at 400 characters and 50 words.
- Expose a small initial subset of Brave parameters rather than the full API surface:
  - `count` / result limit: 1-20.
  - `offset`: 0-9 if pagination is included.
  - `country`, `search_lang`, `ui_lang` as optional config/defaults, not necessarily per-call in the first version.
  - `safesearch`: default `moderate`.
  - `freshness`: optional (`pd`, `pw`, `pm`, `py`, or date range) can be added if easy, but is not required for the first cut.
- Format output conservatively. The MCP server reduces web results to `{ url, title, description, extra_snippets }` and optionally emits FAQ, discussions, news, and videos. Insomnia's first version should return the core web result fields plus provider metadata, and may ignore non-web result buckets unless explicitly requested later.
- Brave's public docs/reference include an LLM Context endpoint (`/res/v1/summarizer/llm_context`) that returns extracted snippets/content with token and per-URL limits. This is useful as a future `WebContext`/enhanced search provider, but should not replace `WebFetch` in the first implementation because it is provider-specific and not a direct URL fetch tool.
- Brave MCP defines a nominal free-plan rate limit of 1 request/second and 15,000/month and does not implement robust self-throttling. Insomnia should at least surface HTTP 429/rate-limit errors clearly; local throttling can be a follow-up unless implementation is cheap.

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
