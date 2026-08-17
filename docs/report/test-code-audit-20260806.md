# Test code audit — 2026-08-06

## Conclusion

The repository has a large test corpus and many high-value authority/state tests, but it is not currently a reliable repository-wide quality gate. Test quantity is not outgrowing production code, yet test bodies are becoming larger and are concentrated in a few inline modules. The Rust workspace suite does not currently compile, the tracked repository has no CI workflow that runs it, and part of the Web UI suite checks source text instead of behavior.

AI authorship cannot be measured reliably from Git metadata. All 400 sampled first-parent commits use the same `hare` author identity, so this audit measures the shape and evolution of tests rather than attempting author attribution.

## Method

Baseline: `HEAD = 46767daf`.

Current size uses tracked Rust, TypeScript, TSX, JavaScript, JSX, and Svelte files. LOC means nonblank physical lines.

Test LOC includes:

- complete files under `tests/` or named `*_test.rs`, `*_tests.rs`, `tests.rs`, `*.test.ts`, or `*.spec.ts`;
- inline Rust `#[cfg(test)] mod ... { ... }` regions.

Rust test count is the number of source declarations carrying `#[test]` or `#[tokio::test]`. Web test count is the count actually reported by `deno task test`. Generated doctests and feature-dependent discovered tests are not included because the full Cargo suite currently fails during compilation.

Historical comparisons use first-parent commit distance rather than commit dates. The latest commit timestamps are later than the Worker environment date, so calendar-time rates would be misleading.

## Current quantity

| Measure | Current value |
|---|---:|
| Production LOC | 166,680 |
| Test LOC | 80,455 |
| Test share of code LOC | 32.6% |
| Test LOC / production LOC | 0.483 |
| Rust test declarations | 2,400 |
| Deno tests actually run | 106 |
| Dedicated test-file LOC | 19,253 |
| Inline `cfg(test)` LOC | 61,202 |
| Inline share of test LOC | 76.1% |
| Test-bearing files | 264 / 495 |
| Median test LOC per test-bearing file | 127 |

Language distribution:

| Language | Production LOC | Test LOC |
|---|---:|---:|
| Rust | 146,626 | 76,541 |
| TypeScript | 16,570 | 3,914 |
| TSX | 551 | 0 |
| Svelte | 2,931 | 0 |
| JavaScript | 2 | 0 |

The absence of Svelte test LOC is important: Web tests do not mount Svelte components. The `.test.ts` suite tests reducers, API helpers, WASM behavior, and source text around Svelte files.

### Concentration

The four largest test modules contain 32,879 test LOC, or 40.9% of all test code:

| Test location | Test LOC | Test declarations |
|---|---:|---:|
| `crates/workspace-server/src/server.rs` | 12,334 | 219 |
| `crates/workspace-server/src/store.rs` | 9,091 | 128 |
| `crates/workspace-api/src/lib.rs` | 6,098 | 116 |
| `crates/worker/src/worker.rs` | 5,356 | 102 |

The ten largest locations contain 59.3% of all test LOC. By crate, `workspace-server`, `worker-runtime`, `worker`, and `workspace-api` together contain 57,703 test LOC, or 71.7% of the repository total.

This concentration is partly justified because these crates own authority and lifecycle behavior. It also means fixture changes and internal representation changes can cause broad, difficult-to-diagnose failures.

## Historical trend

| First-parent snapshot | Production LOC | Test LOC | Test share | Rust test declarations |
|---|---:|---:|---:|---:|
| `HEAD` | 166,680 | 80,455 | 32.6% | 2,400 |
| `HEAD~25` | 160,791 | 79,669 | 33.1% | 2,404 |
| `HEAD~50` | 157,560 | 76,568 | 32.7% | 2,402 |
| `HEAD~100` | 155,087 | 75,035 | 32.6% | 2,356 |
| `HEAD~200` | 143,786 | 70,859 | 33.0% | 2,266 |
| `HEAD~400` | 114,352 | 63,702 | 35.8% | 2,189 |
| `HEAD~800` | 71,833 | 44,687 | 38.4% | 1,745 |
| `HEAD~1200` | 47,284 | 36,514 | 43.6% | 1,422 |

Over the latest 400 first-parent commits:

- production LOC grew 45.8%;
- test LOC grew 26.3%;
- Rust test declarations grew 9.6%;
- test share fell from 35.8% to 32.6%;
- 189 of 400 commits, or 47.3%, changed at least one test declaration.

Tests are therefore not growing faster than production code. The concerning trend is different: a coarse test/helper LOC per source test declaration, measured consistently across snapshots, increased from approximately 27.5 at `HEAD~400` to 31.8 now. The suite is becoming heavier per test and more dependent on shared fixture code.

The cleanup immediately before this audit removed 547 nonblank test LOC and 21 Rust test declarations that duplicated prompt/profile/Flow/model resource contents.

## Test-suite health

### Rust workspace

`cargo test --workspace --no-fail-fast` currently fails during compilation before the workspace suite can run.

The immediate failure is stale test code in `crates/llm-engine/tests/parallel_execution_test.rs`:

- tests pass `Vec<ToolOutput>` where production now requires `Vec<Segment>`;
- tests still construct `ToolOutput` with two arguments although the current constructor accepts one;
- a nested vector repeats the same obsolete contract.

This is direct evidence of test drift. A large test corpus is not a quality gate if the aggregate command cannot compile.

### Web workspace

`cd web/workspace && deno task test` succeeds:

- 106 passed;
- 0 failed.

The run is fast, but a meaningful fraction of the Web suite is source inspection. `worker-console.ui.test.ts` and `config-source/editor-state.test.ts` alone contain about 930 LOC and 23 tests that read source files and assert implementation strings. There is no Playwright, Vitest, Testing Library, jsdom, or other component/browser runner in the tracked Web setup.

### Automation and coverage

- No tracked `.github/workflows` or equivalent CI workflow was found.
- `flake.nix` exposes `checks.default = yoi`, which builds the package but does not run Cargo or Deno tests.
- No active `cargo llvm-cov`, tarpaulin, grcov, codecov, or mutation-test configuration was found.

The repository documentation describes tests as required, but the repository itself does not enforce a green aggregate test gate or measure exercised behavior.

## Quality tendencies

### High-value behavior tests

The strongest tests use real SQLite stores, in-process routers, Runtime brokers, WASM artifacts, or lifecycle state and verify externally meaningful authority boundaries. Representative examples include:

- `workspace-server/config_source.rs::invalid_candidate_is_never_persisted`;
- `workspace-server/config_source.rs::stale_expected_revision_is_rejected`;
- `workspace-server/store.rs::schema_v27_rebuild_rejects_cross_workspace_assignment_repository_drift`;
- `workspace-server/store.rs::worker_spawn_operation_retry_allows_same_reserved_workdir`;
- `workspace-server/server.rs::ticket_assignment_spawn_requires_inprogress_before_runtime_side_effects`;
- `workspace-server/server.rs::destructive_worker_remove_rejects_header_spoof_without_source_proof`;
- `worker-runtime/runtime.rs::scoped_runtime_worker_subscription_hides_other_workspaces`;
- `worker-runtime/runtime.rs::restore_does_not_redispatch_spawn_initial_submit`;
- `ticket/sqlite_schema.rs::migration_rejects_constraint_drift_before_marker_update`;
- `web/workspace/test/config-source/wasm-parity.test.ts`;
- `web/workspace/src/lib/workspace/sidebar/worker-subscription.test.ts`.

These tests protect state ordering, idempotency, workspace isolation, migration atomicity, stale-revision rejection, and information disclosure. They make a real contribution to quality.

### Low-value and brittle tests

The main low-value pattern is implementation/source-shape monitoring presented as regression testing:

- `worker-console.ui.test.ts` checks route strings, assignments such as `nextReloadToken += 1`, CSS imports, and Svelte source fragments without mounting the component;
- `config-source/editor-state.test.ts` checks `$state.raw` and `untrack` text rather than demonstrating that the editor avoids an effect loop;
- some schema tests enumerate complete table/column/index/FK shapes rather than testing the migration behavior that depends on them;
- provider fixture tests allow missing usage fields to pass with a warning, weakening semantic validation despite maintaining fixtures.

These tests can block harmless refactors while failing to prove the user-visible or authority behavior they are named after.

### Maintainability and flakiness risks

- 76.1% of test LOC lives inline in production source files.
- Four files own 40.9% of all test LOC.
- Large shared fixtures mix router wiring, fake Runtime behavior, temporary Git repositories, DB setup, and domain assertions.
- 48 explicit sleep calls occur in test regions across 14 Rust files; `worker/tests/controller_test.rs` contains 19 of them.
- Parallel execution tests use elapsed-time thresholds and can depend on scheduler load.
- Some system-prompt tests retain temporary directories with `std::mem::forget`, leaving environment-cleanup risk.
- Runtime tests frequently manipulate internal snapshots and locks directly, which provides reach but creates implementation coupling.

## Coverage gaps

The largest gaps are at cross-component boundaries:

1. Web Console route reuse and shared WebSocket behavior is checked through reducers and source strings, not a mounted component with a fake protocol client.
2. Config editor source monitoring does not demonstrate generation fencing, stale-response rejection, debounced diagnostics, or commit-button state transitions in the UI.
3. Merge Request completion has strong store-level authority tests but limited provider-level Git integration for stale target refs, non-fast-forward/conflict outcomes, and approved source reachability.
4. Runtime, Server, and Browser multiplexer tests are split by layer; a minimal end-to-end subscription lifecycle is missing.
5. LLM provider fixtures do not consistently fail closed on missing usage, finish-reason, streaming-delta, and tool-call semantics.

## Assessment

The test corpus is not merely write-only: a substantial part of it protects difficult authority and lifecycle contracts that would otherwise regress. The concern is nevertheless valid because quantity is being used without a reliable feedback system.

The primary failure is not “too many tests.” It is that the repository lacks these controls:

- an always-green aggregate gate;
- a distinction between behavior tests and source-shape guards;
- a maintenance owner or deletion rule for stale tests;
- flake/runtime monitoring;
- coverage or mutation evidence for critical invariants.

Without those controls, additional AI-generated tests can increase review and maintenance cost without increasing defect detection.

## Recommended policy

1. Restore a green workspace command first, then run it in tracked CI. Do not accept new tests while the aggregate suite is uncompilable unless they repair the baseline.
2. Require every new test to name an independent observable invariant. A test that only repeats prompt/config/source data should be rejected.
3. Prefer tests at the narrowest authoritative boundary: real DB transaction, public resolver, router, broker, reducer, or provider adapter.
4. Replace Web source-inspection tests with component behavior tests using a fake protocol/fetch boundary. Delete source checks when no executable behavior boundary exists yet.
5. Split giant test modules by authority area and share only typed fixture builders. Do not share mutable scenario scripts that hide setup and expected effects.
6. Replace fixed sleeps and elapsed thresholds with barriers, channels, paused time, or explicit event acknowledgements where possible.
7. Track quality metrics, not test-count targets:
   - aggregate suite green rate;
   - flaky/retry rate;
   - critical invariant coverage by domain;
   - mutation kill rate for selected authority modules;
   - production defects that had or lacked a regression test;
   - test LOC and runtime as costs, not goals.
8. Periodically delete tests that no longer protect an independent invariant. Test deletion should be treated as maintenance, not loss of quality.
