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

<!-- event: implementation_report author: hare at: 2026-05-31T01:57:37Z -->

## Implementation report

Implementation report from coder Pod `codex-gpt55-context-coder-20260531`:

- Branch: `codex-gpt55-effective-context-window`
- Commit: `ed3e97c22a8e21dce9223114526932a11b268589` (`provider: use codex gpt55 effective context`)
- Updated `resources/models/builtin.toml` so `codex-oauth/gpt-5.5` records the effective Codex OAuth route limit directly as `context_window = 272000` and no longer has `max_context_window`.
- Added a nearby comment explaining that OpenAI documents GPT-5.5 as 1.05M context, but Codex OAuth / ChatGPT backend access is effectively limited around 272k.
- Kept default profile unchanged and did not change OpenRouter `openai/gpt-5.5`.
- Preserved `max_context_window` support for inline/config overrides and updated tests accordingly.

Validation reported by coder:

- `cargo fmt --check` passed
- `cargo test -p provider catalog` passed
- `cargo test -p manifest model` passed
- `cargo check -p provider -p manifest` passed
- `./tickets.sh doctor` passed
- `git diff --check` passed

Unresolved issues: none.


---

<!-- event: review author: hare at: 2026-05-31T01:57:38Z status: approve -->

## Review: approve

External review by reviewer Pod `codex-gpt55-context-reviewer-20260531`: approve.

Reviewer summary:

- The change is correctly scoped to `codex-oauth/gpt-5.5` and relevant tests.
- `context_window = 272000` now represents the effective Codex OAuth route limit directly.
- The comment explains the distinction between OpenAI's advertised 1.05M context window and the observed/known Codex OAuth effective limit.
- `max_context_window` support remains available for inline/config cases.
- Default profile and OpenRouter `openai/gpt-5.5` were not changed.

Blockers: none.


---
