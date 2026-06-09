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
