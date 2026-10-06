# T-702 validation: intermittent stale Ticket checker assertion

The full `yoi-workspace-server` library suite intermittently fails
`server::tests::ticket_checker_suppresses_stale_and_clean_results_and_diagnoses_invalid_output`
at its post-stale-result `assert!(execution.take_inputs().is_empty())`. The same unchanged
assertion passed in some isolated/full runs and failed in others during T-702 validation.

To separate this from Worker operation reception, I archived original base
`b96bd110c623fafdd7e8df1b153e8c640c793abe` under the ignored `target/t702-baseline`
directory without moving/replacing refs or discarding the Ticket Workdir. A first
shared-target build failed with stale dependency-interface artifacts and was **not**
baseline evidence. I then used an isolated `CARGO_TARGET_DIR=target/t702-baseline-build`
with `CARGO_BUILD_JOBS=3`. The baseline compiled successfully; its isolated checker test
passed. Its full library run with `--test-threads=4` reproduced the same assertion:
**599 passed, 1 failed, 600 total** (79.19s), at baseline `server.rs:44331`.
The post-review T-702 fix's full library run reported **611 passed, 1 failed, 612 total**
(79.01s), at the corresponding `server.rs:44266` assertion. This demonstrates an inherited
intermittent validation failure, not a green suite or proof of its root cause.
A subsequent post-review-fix whole-crate run, with the same implementation and no skips,
passed **612 library + 11 binary tests and doc tests** (library 81.24s). That successful
run does not erase the failed runs or resolve the underlying checker race.

The Worker reception fix's 12 operation-boundary tests, 5 WorkerRemove tests (including
constructor-installed embedded dispatcher, target-bound proof, resource-key normalization,
running/pinned refusals, retention and catalog removal), root `cargo check`, formatting and
diff checks passed. The checker implementation/test was not changed or skipped to conceal
the failure. Follow-up should instrument competing checker-job dispatch and deterministic
execution-input consumption so this assertion has an explicit synchronization boundary.
