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
