---
id: 20260528-152959-web-search-fetch-tools-review
slug: web-search-fetch-tools
title: Review for WebSearch and WebFetch tools
status: reviewed
kind: review
created_at: 2026-05-28T15:29:59Z
updated_at: 2026-05-29T09:28:00Z
reviewer: insomnia-system
---

## Review summary

Reviewed implementation branch `work/web-search-fetch-tools` in worktree `/home/hare/Projects/insomnia/.worktree/web-search-fetch-tools`.

The implementation adds normal built-in function tools `WebSearch` and `WebFetch`, not provider-hosted OpenAI/Codex tools. `WebSearch` uses Brave Search API with environment-variable API key configuration, query/limit/offset validation, and bounded JSON output. `WebFetch` uses an independent HTTP client with URL/scheme/host/IP policy, redirect revalidation, timeout and byte limits, content-type checks, and HTML/text/JSON/XML-ish rendering. Both tools are registered through the existing built-in tool path and fail closed when web access is disabled or search is unconfigured.

One blocking issue was found and fixed: Brave WebSearch initially had no request timeout and read the provider response body without a size bound. The amendment adds typed search timeout configuration and bounded response reading.

The implementation keeps Codex hosted web search out of scope, which matches the ticket decision.

## Validation

Reviewer ran:

- `cargo fmt --check`
- `cargo test -p tools --no-default-features`
- `cargo test -p manifest --no-default-features`
- `cargo check -p pod --no-default-features`
- `cargo check -p tui --no-default-features`
- `git diff --check develop...HEAD`

All passed. The only compiler warnings observed were pre-existing dead-code warnings under no-default feature checks.

## Judgment

Approved after amendment.
