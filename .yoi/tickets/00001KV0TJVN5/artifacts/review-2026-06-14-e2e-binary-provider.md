## Review: approve

Decision: approve for Ticket `00001KV0TJVN5`.

Evidence reviewed:
- Ticket intent/acceptance criteria require default E2E setup to build `yoi` with `cargo build -p yoi --features e2e-test --bin yoi`, then direct-spawn the produced binary, while preserving `YOI_E2E_BIN` override and existing panel E2E behavior.
- `tests/e2e/src/lib.rs` now resolves `yoi_binary()` through a `OnceLock`-cached `BinaryProviderInfo`. The default path runs `${CARGO:-cargo} build -p yoi --features e2e-test --bin yoi` from the workspace root and returns `target/{debug|release}/yoi`; the override path validates and uses `YOI_E2E_BIN` without invoking the cargo-build provider.
- PTY execution remains `Command::new(&config.binary).arg("panel")`; `cargo run` is not in the process-under-test path.
- `PanelHarness::spawn` and fixture `run_yoi_capture` both call `env_clear()` and then set only explicit fixture/test variables. `PATH` and provider credentials are not allowlisted. `YOI_POD_RUNTIME_COMMAND` is set to the resolved binary path, so tested subprocesses do not need host `PATH`.
- Diagnostics/artifacts include provider/build/env policy in `target/e2e-artifacts/binary-provider.json`, panel `run.json`, and fixture `fixture-commands.jsonl`.
- Existing mouse-capture guard (`expect_mouse_capture_enabled` / SGR 1000+1006 tracking), background-task quit barrier assertions, and `e2e-test` production boundary code were not weakened by this diff.

Validation:
- Reviewer reran `git diff --check a4df9754..HEAD` — passed.
- Reviewer reran `cargo test -p yoi-e2e --features e2e tested_yoi_env_policy_is_env_clear_allowlist -- --nocapture` — passed.
- Also accepted Orchestrator-reported full validation, including fmt/check, `cargo check -p yoi-e2e --all-targets --features e2e`, default panel E2E with host provider env present, and `YOI_E2E_BIN` override panel E2E with host provider env present — all reported passed.

Risks / follow-up:
- No blocking issues found. The cargo build provider intentionally still uses build-tool environment; tested `yoi` subprocesses are isolated.