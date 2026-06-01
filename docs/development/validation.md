# Validation

Validation should match the change. Do not run expensive broad checks just to look thorough, but do not skip checks that prove the changed boundary still works.

## Docs-only changes

Minimum checks:

```sh
./tickets.sh doctor
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
./tickets.sh doctor
```

Use work item review to verify that the implementation satisfies the ticket, not only that the diff looks plausible.

## Current limitation

End-to-end tests that spawn real processes are not yet designed. When changing Pod restoration, socket behavior, or orchestration, compensate with targeted unit/integration tests and concrete manual evidence in the implementation report.
