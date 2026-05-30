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
