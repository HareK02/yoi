---
id: 20260530-204045-webfetch-readable-extraction
slug: webfetch-readable-extraction
title: WebFetch: extract main HTML content with lightweight readability
status: open
kind: task
priority: P2
labels: [web, tools, html]
created_at: 2026-05-30T20:40:45Z
updated_at: 2026-05-30T20:54:26Z
assignee: null
legacy_ticket: null
---

## Background

`WebFetch` currently returns bounded, safety-checked content but the HTML path is still close to raw page text: it strips tags with a small local formatter and does not try to isolate the article/main content. For LLM use, a reader-mode style extraction layer is more useful than raw boilerplate-heavy page text.

`readability-js` was investigated but brings QuickJS / bundled JavaScript dependency weight. The desired direction is a lightweight pure-Rust extraction backend with fallback to the current `html_to_text` behavior.

Reference implementations checked out with ghq under `.worktree/ghq-root/` for planning:

- `github.com/quambene/readability-rs` — crate `readability-rs`, MIT, small arc90-style extractor (`Readable { title, content, text }`).
- `github.com/theiskaa/readabilityrs` — crate `readabilityrs`, Apache-2.0, larger Mozilla Readability port with metadata/markdown support.
- `github.com/readable-app/readability.rs` — crate `readable-readability`, MIT, kuchiki-based extractor but sparse docs and older maintenance surface.

## Requirements

- Keep `WebSearch` and `WebFetch` as separate tools. Do not add an automatic summarization/research tool in this ticket.
- Add a reader-mode extraction path for HTML responses in `WebFetch`.
- Use a pure-Rust dependency or local extraction implementation; do not use `readability-js`, QuickJS, Node, Python, or subprocess-based extraction.
- Prefer the lightweight `readability-rs` crate if it builds cleanly and produces usable `title` + main `text`; escalate if the crate is incompatible or obviously too low-quality for the included fixtures.
- Preserve the current network safety behavior: configured provider requirement, private/local host rejection, bounded redirects, response size limits, binary rejection, output truncation, and untrusted-content warning semantics.
- Preserve fallback behavior. If readability extraction fails or returns empty/too-short text, return HTML text produced by the existing local fallback rather than failing the entire fetch.
- Structure HTML fetch output so the LLM can distinguish extraction metadata from document content. At minimum include:
  - extraction method (`readability` or fallback name)
  - fallback indicator / reason when applicable
  - title when available
  - main text
  - existing fetch metadata such as URL/final URL/status/content type/truncation
- Keep HTML returned to the LLM as text by default. Do not expose full extracted HTML unless there is a clear existing output field need.
- Add focused tests with small HTML fixtures covering:
  - article/main content is preferred over nav/footer/sidebar boilerplate
  - fallback is used when readability extraction is not useful
  - output remains bounded/truncated under the existing output limit

## Non-goals

- Provider expansion or changes to `WebSearch` provider selection.
- LLM-generated summaries inside `WebFetch`.
- Browser rendering, JavaScript execution, or dynamic page support.
- Large benchmark suite or exhaustive readability quality comparison.
- Public API/protocol changes beyond the tool result JSON shape.

## Implementation plan

1. Add the selected pure-Rust readability dependency to `crates/tools`.
2. Introduce a small internal HTML extraction helper, e.g. `extract_html_document(html, base_url, output_limit)`, wrapping readability success and fallback.
3. Update the `ContentKind::Html` branch in `WebFetch` rendering to use the helper.
4. Keep existing `html_to_text` as fallback and testable utility.
5. Update tests in `crates/tools/src/web.rs` or a focused tools test module.
6. Validate with formatting, focused tools tests, and broader checks appropriate to the dependency change.

## Acceptance criteria

- `WebFetch` HTML responses prefer extracted main content over navigation/footer/sidebar boilerplate in tests.
- `WebFetch` still returns useful bounded text when readability extraction fails or is empty.
- Tool output clearly reports extraction method and fallback status.
- No JavaScript engine/runtime dependency is introduced.
- `Cargo.lock` and Nix cargo hash implications are handled or explicitly reported.
- `cargo fmt --check`, focused tools tests, `cargo check -p tools`, and `./tickets.sh doctor` pass or any failure is clearly reported as unrelated/pre-existing.
