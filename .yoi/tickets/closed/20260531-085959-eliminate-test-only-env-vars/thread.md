<!-- event: create author: tickets.sh at: 2026-05-31T08:59:59Z -->

## Created

Created by tickets.sh create.

---

<!-- event: review author: hare at: 2026-05-31T10:03:55Z status: approve -->

## Review: approve

External reviewer: `eliminate-test-env-vars-reviewer-20260531`
Reviewed implementation commit: `e64a5595956c970b090cdce851cc962e92723a97` (`test: remove test-only env vars`)
Verdict: approve

Summary:
- WebSearch tests no longer generate or depend on `INSOMNIA_TEST_*` API-key env names.
- The request/search implementation was split so tests can inject an API key directly into a private helper while production still reads the configured `web.search.api_key_env` and fails closed for missing/empty values.
- `docs/environment.md` no longer presents test-only env vars as a supported surface.

Requirements mapping:
- No active non-work-item `INSOMNIA_TEST` references remain.
- No replacement test-only env var was introduced.
- Credential env vars and `INSOMNIA_POD_COMMAND` were not removed by this ticket.
- WebSearch production behavior and network safety boundaries are preserved.

Blockers: none.

Non-blocking follow-up:
- A future public-path fail-closed test could guard missing/empty `api_key_env`, but this is not required for this ticket.

Validation adequacy:
- Coder validation covered fmt, tools tests/check, ticket doctor, diff check, and residual `INSOMNIA_TEST` grep.
- Reviewer performed read-only diff/source/docs/grep review and did not rerun tests.


---

<!-- event: close author: hare at: 2026-05-31T10:04:28Z status: closed -->

## Closed

Removed test-only environment-variable usage from active code.

Implementation:
- Removed `INSOMNIA_TEST_*` Brave WebSearch test key generation/dependency.
- Split Brave search request execution so tests can inject an API key directly into a private helper.
- Preserved production behavior: WebSearch still reads configured `web.search.api_key_env` and fails closed for missing/empty values.
- Updated `docs/environment.md` so test-only env vars are not listed as supported surface.

Review:
- External reviewer `eliminate-test-env-vars-reviewer-20260531` approved implementation commit `e64a5595956c970b090cdce851cc962e92723a97`.

Validation after merge:
- `cargo fmt --check`
- `cargo test -p tools`
- `cargo check -p tools` (passed with unrelated existing `llm-worker` dead_code warning)
- `./tickets.sh doctor`
- `git diff --check`
- `git grep -n "INSOMNIA_TEST" -- ':!work-items' || true` produced no active references.


---
