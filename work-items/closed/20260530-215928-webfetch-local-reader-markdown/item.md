---
id: 20260530-215928-webfetch-local-reader-markdown
slug: webfetch-local-reader-markdown
title: WebFetch: replace readability dependency with Markdown-preserving local reader
status: closed
kind: task
priority: P2
labels: [web, tools, html]
created_at: 2026-05-30T21:59:28Z
updated_at: 2026-05-30T22:21:39Z
assignee: null
legacy_ticket: null
---

## Background

`webfetch-readable-extraction` added `readability-rs` to improve `WebFetch` HTML output. It proved the direction, but the next design step is to own the reader behavior instead of depending on an article extractor that flattens links to plain text.

For LLM research workflows, article text without links is lossy: links inside the readable body often point to RFCs, docs, downloads, related pages, or citations that the agent must be able to follow. At the same time, navigation/sidebar content should be omitted by default, while still being discoverable when the page is documentation/book-like and navigation links are important.

## Requirements

- Replace the `readability-rs` dependency with a local, pure-Rust HTML reader extractor in `crates/tools`.
- Keep `WebSearch` and `WebFetch` separate. Do not add summarization or research orchestration in this ticket.
- `WebFetch` HTML output should be Markdown-ish text, not plain text:
  - preserve inline links as `[label](absolute-url)`;
  - preserve useful headings/lists/paragraph breaks enough for LLM readability;
  - do not expose full HTML by default.
- Add optional `include_navigation: Option<bool>` to `WebFetchInput`, defaulting to `false`.
- Detect navigation-like content (`nav`, sidebar/toc/menu/breadcrumb-ish class/id/role, previous/next chapter areas, etc.) generically.
  - With `include_navigation=false`, omit navigation from the main text by default.
  - If navigation was detected and omitted, include metadata/notice in the tool result such as “navigation was detected and omitted; re-run with include_navigation=true if navigation/sidebar links are needed.”
  - With `include_navigation=true`, include a bounded `## Navigation` section containing navigation links rendered as Markdown.
- Treat reader failure as a page-selection/readability signal, not as a second hidden reader mode:
  - report `readable=false` or equivalent metadata/reason when no useful main content was selected;
  - fallback text may remain as diagnostic last resort, but metadata must make clear it is fallback/raw-ish output.
- Preserve current WebFetch safety behavior:
  - configured provider requirement;
  - private/local host rejection;
  - bounded redirects, response size, and output size;
  - binary rejection;
  - untrusted-content warning semantics.
- Preserve output bounding for both main text and navigation content.
- Avoid site-specific branches for mdBook/docs.rs/rustdoc/etc.; use generic DOM/tag/class/id/role heuristics only.

## Non-goals

- Firefox/Mozilla Readability compatibility.
- JavaScript execution, browser rendering, QuickJS, Node, Python, or subprocess extraction.
- Search result ranking changes or provider expansion.
- LLM summarization inside `WebFetch`.
- Exhaustive benchmark/quality suite.

## Implementation guidance

- Prefer using a lightweight DOM parser dependency already implied by the current dependency graph if possible (`html5ever` / rcdom or similar). It is acceptable to retain such parser dependencies directly while removing `readability-rs`.
- Build a small local extractor with clear stages:
  1. parse HTML;
  2. classify nodes as navigation/skipped/main candidates;
  3. select the best main candidate using simple scoring (text length, paragraph count, link density, positive tags like `main`/`article`, negative class/id words);
  4. render selected content as bounded Markdown with absolute links;
  5. optionally render bounded navigation links under `## Navigation`.
- Keep the existing simple `html_to_text` path only as explicit diagnostic fallback when local reader extraction cannot find useful content.
- Keep result JSON compatibility where practical, but update `html_extraction` metadata to expose method, readable status, navigation status, fallback status/reason, and title when available.

## Acceptance criteria

- `readability-rs` is removed from direct dependencies and no JavaScript runtime dependency is introduced.
- HTML article fixture renders body links as Markdown `[label](absolute-url)`.
- Navigation/sidebar/footer are omitted from main text by default.
- When navigation is omitted, result metadata or notice clearly says navigation was detected and can be included via `include_navigation=true`.
- With `include_navigation=true`, bounded navigation links appear under a separate `## Navigation` section.
- Link-heavy navigation-only pages are not misreported as successfully readable main content.
- Existing safety and bounds tests continue to pass.
- Focused tests cover link preservation, navigation omission notice, navigation inclusion, reader failure/fallback metadata, and truncation/bounds.
- `cargo fmt --check`, focused tools tests, `cargo check -p tools`, `./tickets.sh doctor`, `git diff --check`, and Nix build/hash handling pass or failures are clearly reported.
