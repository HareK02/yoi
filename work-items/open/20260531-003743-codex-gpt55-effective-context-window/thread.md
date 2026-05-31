<!-- event: create author: tickets.sh at: 2026-05-31T00:37:43Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-31T00:38:23Z -->

## Plan

Implementation plan:

1. Update only the provider-specific builtin catalog entry for `codex-oauth/gpt-5.5`: set `context_window = 272000`, remove `max_context_window`, and add a clear comment about generic OpenAI docs vs Codex OAuth effective limit.
2. Keep `resources/profiles/default.lua` unchanged.
3. Adjust tests so builtin catalog expectations use effective `context_window`, while `max_context_window` behavior remains covered by an inline/config-specific test if the field remains supported.
4. Validate with focused provider/manifest tests plus `cargo fmt --check`, `./tickets.sh doctor`, and `git diff --check`.


---
