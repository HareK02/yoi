# Validation

Validation should match the change. Do not run expensive broad checks just to look thorough, but do not skip checks that prove the changed boundary still works.

## Docs-only changes

Minimum checks:

```sh
git diff --check HEAD
```

Useful stale-surface sweeps:

```sh
STALE_DOC_PATTERN='TODO[.]md|obsolete doc dirs|old ticket surface'
rg -n "$STALE_DOC_PATTERN" README.md docs crates/*/README.md --glob '!docs/report/**'
```

## Code changes

Follow `AGENTS.md` and the [test strategy](rust-testing-strategy.md). Start with the smallest target/filter that proves the changed contract:

```sh
cargo test -p <crate> --lib <test-or-module-filter>
cargo test -p <crate> --test <test-target> <test-filter>
```

Before completion, run the root compile check and the changed crate's full tests, plus formatting and diff checks:

```sh
cargo check
cargo test -p <changed-crate>
cargo fmt --all -- --check
git diff --check HEAD
```

Unit, component, and integration suites are ordinary tests. Do not exclude a named integration target merely because it uses temporary SQLite, loopback/IPC, or an isolated child process. Include affected product features and cross-crate integration suites beyond the changed crate when they own the connection being changed. `cargo check` does not execute tests, and `--lib` alone does not run external integration-test targets.

Workspace-wide tests and heavyweight builds are selected when the changed boundary requires them, not as a substitute for identifying the relevant suites. Avoid repository-wide formatting churn when a validation failure is caused by pre-existing unrelated formatting.

## Web UI changes

Rendered Web UI changes require implementer-owned visual review in addition to type check, linter, and test. Follow [`ui-ux/visual-review.md`](ui-ux/visual-review.md).

Minimum evidence includes:

- before/after capture under the relevant persona, viewport, theme, and data state;
- the implementer’s own inspection of grouping, separator meaning, spacing, wrapping, overflow, responsive order, and visible errors;
- `review-context.json` inspection;
- disposition of every observed problem;
- artifact paths and a final `pass | fail` result.

A successful capture command, HTTP 200, screenshot generation, or later Reviewer inspection does not replace this validation.

## Work item checks

Use the authorized Ticket tools and boundaries described in `AGENTS.md`. Work item review verifies that the implementation satisfies the Ticket, not only that the diff looks plausible; it does not replace executable tests.

## E2E validation

Product-assembly journeys belong to the workspace `tests/e2e` package and use its shared harness. Add or extend these tests for an explicitly selected journey that ordinary component/integration tests cannot prove. A crate-local test using an isolated child process is not E2E merely because it crosses a process boundary.

E2E has a dedicated opt-in entry point:

```sh
cargo test -p yoi-e2e --features e2e --test <target> <test-filter>
```

Select the target for the affected journey. Its suite documentation must describe required binaries/build features, runtime prerequisites, isolation and cleanup, timeouts, and diagnostic artifacts. Do not substitute a live developer deployment or create a second product-launch harness in an individual crate.

The package is outside workspace `default-members`, and its test targets require the `e2e` feature. An ordinary root test run does not execute these targets; even a workspace-wide test run without the feature does not prove E2E behavior. Record the selected target and actual result, or state that E2E was not run.

## CI execution policy

Suite ownership includes execution selection, not only source placement:

- Ordinary unit/component/integration suites belong in normal validation jobs. Jobs must include named integration targets and the product-feature configurations they claim to validate; a `--lib` job is not whole-crate coverage.
- E2E belongs in a separate job that explicitly selects the package, feature, and journey targets. State its trigger policy: which changes require it and whether it also runs manually or on a schedule. Opt-in locally must not mean an unowned suite that is never exercised.
- Suites with special platform, tool, or privilege requirements need a suitable job and an explicit selection rule. Keep the portable portion in normal validation rather than hiding an entire suite behind the exceptional case.
- Timeouts, concurrency limits, resource cleanup, and failure artifacts belong to the shared harness/runner configuration. Do not rely on undocumented local setup.

When adding or changing a suite, identify the job that executes it and update its selection if needed. If execution is not yet wired, record that gap; a policy statement or a successful local command is not evidence that CI runs the suite.
