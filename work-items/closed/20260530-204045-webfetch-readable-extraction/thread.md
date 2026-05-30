<!-- event: create author: tickets.sh at: 2026-05-30T20:40:45Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-30T20:41:21Z -->

## Plan

Planning note:

- ghq checkouts for prior art were placed under `.worktree/ghq-root/` so they stay inside the repository write scope and under the ignored `.worktree/` area.
- `readability-js` is intentionally excluded from the implementation path because it pulls in QuickJS/rquickjs and bundled JavaScript.
- Candidate preference for this ticket is `readability-rs` first because it is small, MIT licensed, and exposes a simple `extract` API returning `title`, extracted HTML, and text. If it fails to build or extraction is unusable on the ticket fixtures, the coder should stop and report rather than silently switching to a heavier dependency.
- `readabilityrs` is the heavier pure-Rust backup candidate and useful for reference, but adopting it changes the dependency footprint more significantly.


---

<!-- event: implementation_report author: hare at: 2026-05-30T20:54:26Z -->

## Implementation report

Implementation report from coder Pod `webfetch-readable-coder-20260530`:

- Branch: `webfetch-readable-extraction`
- Commit: `7906ca532666669417c20d831a08103c2f0f80dd` (`web: extract readable html content`)
- Changed files: `Cargo.lock`, `crates/tools/Cargo.toml`, `crates/tools/src/web.rs`, `package.nix`
- Added `readability-rs = 0.5.0` to `tools` and updated Nix cargo hash.
- Added a WebFetch HTML extraction helper that uses readability for main text when useful and falls back to existing `html_to_text` when readability fails or returns too-short text.
- Added `html_extraction` metadata with method/fallback/reason/title and kept output bounded.
- Full extracted HTML is not returned.

Validation reported by coder:

- `cargo fmt --check` passed
- `cargo test -p tools web` passed (10 passed)
- `cargo check -p tools` passed, with only existing `llm-worker` dead_code warning
- `./tickets.sh doctor` passed
- `git diff --check` passed
- `nix build .#insomnia` passed

Unresolved issues: none.


---

<!-- event: review author: hare at: 2026-05-30T20:54:26Z status: approve -->

## Review: approve

External review by reviewer Pod `webfetch-readable-reviewer-20260530`: approve.

Summary:
- The change adds a pure-Rust `readability-rs` extraction path for `WebFetch` HTML responses.
- HTML responses use reader-mode text when extraction is useful and fall back to existing local `html_to_text` otherwise.
- Output JSON includes separate `html_extraction` metadata plus document `text`, while preserving fetch metadata and untrusted-content warning.

Requirements check:
- `WebSearch` / `WebFetch` separation preserved.
- Pure Rust dependency only; no QuickJS, Node, Python, browser, or subprocess path.
- Existing WebFetch safety behavior remains in place.
- Fallback behavior exists for readability errors and too-short/empty text.
- Output separates extraction metadata from text.
- Full extracted HTML is not exposed.
- Tests cover fallback metadata, article/main preference over nav/footer, truncation, and existing WebSearch/fetch safety behavior.
- Dependency and Nix hash changes are reasonable.

Blockers: none.

Non-blocking follow-up:
- Optional future direct test for a stable readability error path; current fallback coverage is sufficient for this ticket.


---

<!-- event: implementation_report author: hare at: 2026-05-30T20:55:12Z -->

## Implementation report

Main workspace validation after merge:

- `cargo fmt --check` passed
- `cargo test -p tools web` passed (10 passed)
- `cargo check -p tools` passed with existing `llm-worker` dead_code warning
- `./tickets.sh doctor` passed
- `git diff --check` passed
- `nix build .#insomnia` passed (with dirty tree warning due to unrelated `.insomnia/workflow/multi-agent-workflow.md` local modification)


---

<!-- event: close author: hare at: 2026-05-30T20:55:13Z status: closed -->

## Closed

Implemented `WebFetch` HTML reader-mode extraction with pure-Rust `readability-rs`, preserving existing safety checks and fallback to local `html_to_text`. Output now reports `html_extraction` metadata and bounded main text without exposing extracted HTML by default. Reviewed externally and approved; validation passed including focused tools tests and `nix build .#insomnia`.


---
