<!-- event: create author: tickets.sh at: 2026-05-30T21:59:28Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-30T22:00:33Z -->

## Plan

Implementation plan:

1. Replace the current `readability-rs` adapter with a local DOM-based reader extractor scoped to `crates/tools`.
2. Add `include_navigation` to `WebFetchInput`, default false, and thread it only through the HTML render path.
3. Render readable content as Markdown-ish text so inline links remain followable.
4. Detect navigation generically and omit it by default while reporting a notice; include bounded navigation links only when requested.
5. Remove the direct `readability-rs` dependency and update Cargo/Nix lock data.
6. Validate with focused web tests, tools check, doctor, diff check, and Nix build/hash handling.


---

<!-- event: implementation_report author: hare at: 2026-05-30T22:20:54Z -->

## Implementation report

Implementation report from coder Pod `webfetch-local-reader-coder-20260530`:

- Branch: `webfetch-local-reader-markdown`
- Commits:
  - `2a3208b96e151d748fd80d5a1c4236ba616b3c6e` `web: render readable html as markdown`
  - `44262c42365cbba1dad87629c06ce539cbd25105` `web: harden reader navigation fallbacks`
- Changed files: `crates/tools/src/web.rs`, `crates/tools/Cargo.toml`, `Cargo.lock`, `package.nix`
- Removed direct `readability-rs` dependency and replaced it with a local `html5ever`/rcdom reader implementation.
- Added `include_navigation: Option<bool>` to `WebFetchInput`.
- Renders readable HTML as Markdown-ish text, preserving inline body links as `[label](absolute-url)`.
- Detects generic navigation/sidebar/toc/menu/breadcrumb/pagination/prev-next content, omits it by default with metadata/notice, and includes bounded `## Navigation` when requested.
- Reader failure/fallback is reported with `readable=false`/fallback metadata; fallback output omits detected navigation by default so metadata and text agree.
- Added/strengthened regression tests for link preservation, navigation omission/inclusion, link-heavy main failure, fallback navigation consistency, and navigation truncation.

Validation reported by coder:

- `cargo fmt --check` passed
- `cargo test -p tools web` passed (14 passed)
- `cargo check -p tools` passed
- `./tickets.sh doctor` passed
- `git diff --check` passed
- `nix build .#insomnia` passed

Unresolved issues: none.


---

<!-- event: review author: hare at: 2026-05-30T22:20:54Z status: approve -->

## Review: approve

External review by reviewer Pod `webfetch-local-reader-reviewer-20260530`: approve.

First review requested changes for two blockers:

1. link-heavy `body` / `main` could be accepted as readable main content;
2. fallback could claim navigation omission while returning detected navigation text.

Follow-up commit `44262c42365cbba1dad87629c06ce539cbd25105` resolved both:

- `candidate_score` rejects high link density for all candidate tags, including `body` and `main`;
- fallback text is generated through the DOM reader path so detected navigation is omitted by default when `include_navigation=false`;
- metadata aligns with included/omitted navigation state;
- tests cover link-heavy main, fallback nav omission consistency, strengthened omitted nav labels, and navigation truncation metadata.

Reviewer found no new blocker. Reported validation is adequate.


---

<!-- event: implementation_report author: hare at: 2026-05-30T22:21:39Z -->

## Implementation report

Main workspace validation after merge:

- `cargo fmt --check` passed
- `cargo test -p tools web` passed (14 passed)
- `cargo check -p tools` passed with existing `llm-worker` dead_code warning
- `./tickets.sh doctor` passed
- `git diff --check` passed
- `nix build .#insomnia` passed (with dirty tree warning due to existing `.insomnia/workflow/multi-agent-workflow.md` local modification and open ticket lifecycle files)


---

<!-- event: close author: hare at: 2026-05-30T22:21:39Z status: closed -->

## Closed

Replaced the `readability-rs` WebFetch HTML extraction path with a local pure-Rust DOM reader that renders Markdown-ish main content and preserves inline links as absolute Markdown links. Added optional `include_navigation`, default navigation omission notices, bounded navigation inclusion, readable/fallback metadata, and regression coverage. External review approved after blocker fixes; validation passed including focused tools tests and Nix build.


---
