# Validation

Validation should match the change. Do not run expensive broad checks just to look thorough, but do not skip checks that prove the changed boundary still works.

## Docs-only changes

Minimum checks:

```sh
yoi ticket doctor
git diff --check
```

Useful stale-surface sweeps:

```sh
STALE_DOC_PATTERN='TODO[.]md|obsolete doc dirs|old ticket surface'
rg -n "$STALE_DOC_PATTERN" README.md docs crates/*/README.md --glob '!docs/report/**'
```

## Code changes

Prefer targeted tests first, then broader checks when the touched boundary is central.

Common checks:

```sh
cargo test --workspace
cargo test -p <crate>
cargo check --workspace
```

Avoid repository-wide formatting churn when a validation failure is caused by pre-existing unrelated formatting.

## Work item checks

Run:

```sh
yoi ticket doctor
```

Use work item review to verify that the implementation satisfies the ticket, not only that the diff looks plausible.

## E2E validation

Real-process E2E tests are opt-in. Do not add or extend E2E coverage unless the work explicitly requires the E2E test to be designed and implemented. Otherwise validate the narrower parser, API, protocol, authority, or runtime boundary.
