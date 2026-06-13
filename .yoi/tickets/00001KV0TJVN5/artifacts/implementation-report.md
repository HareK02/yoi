Implementation report for Ticket 00001KV0TJVN5

Files changed:
- `tests/e2e/src/lib.rs`
  - Added a cached e2e binary provider using `OnceLock`.
  - Preserves `YOI_E2E_BIN=<path>` as the explicit override and skips the default cargo build provider in that path.
  - Default path runs `${CARGO:-cargo} build -p yoi --features e2e-test --bin yoi` from the workspace root, then returns the direct `target/{profile}/yoi` binary path for PTY spawning.
  - Writes `target/e2e-artifacts/binary-provider.json` and emits diagnostics with provider, build command, binary path, and tested-subprocess env policy.
  - Expanded command-failure diagnostics to include command args.
  - Follow-up: isolated tested `yoi` subprocess environments in both `PanelHarness::spawn` and fixture setup `run_yoi_capture` with `env_clear()` plus explicit allowlists only.
  - Follow-up: recorded env policy in `run.json`, `binary-provider.json`, and per-fixture `fixture-commands.jsonl` artifacts.
  - Follow-up: added a regression assertion that tested-subprocess policies use `env_clear`, do not allow `PATH`, and default-deny provider credentials (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`) and secret-like patterns.
  - Follow-up: relative `YOI_E2E_BIN` values are resolved against the workspace root and must exist, so tested subprocess launch does not rely on `PATH` lookup.
- `tests/e2e/tests/panel.rs`
  - Updated panel tests to use the fallible cached binary provider.

Env isolation policy:
- Cargo build provider remains a build-tool command and is not treated as the tested `yoi` subprocess.
- Tested `yoi` fixture setup commands receive only: `HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME`, `YOI_POD_RUNTIME_COMMAND`.
- Tested `yoi panel` commands receive only: fixture `HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME`, `TERM`, `YOI_TUI_TEST_EVENTS`, `YOI_POD_RUNTIME_COMMAND`, and `YOI_TUI_TEST_HOLD_BACKGROUND_TASK` when used.
- `PATH` is intentionally not passed to tested `yoi` subprocesses; the harness launches the already-resolved binary path directly.
- Host provider credentials / token / secret-like environment variables are default-denied. Future provider/LLM E2E should use fixture providers, canned servers, or explicit test env instead of inheriting host credentials.

Validation:
- `cargo fmt --check` — passed.
- `git diff --check` — passed.
- `cargo check -p yoi-e2e --all-targets --features e2e` — passed.
- `cargo test -p yoi-e2e --features e2e tested_yoi_env_policy_is_env_clear_allowlist -- --nocapture` — passed.
- `unset YOI_E2E_BIN && OPENAI_API_KEY=host-secret ANTHROPIC_API_KEY=host-secret GEMINI_API_KEY=host-secret cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed; default provider built the current `yoi` binary and tested `yoi` subprocesses used isolated env policy artifacts. Host provider env was present for the harness but is not inherited by tested `yoi` subprocesses because `env_clear()` is applied before the allowlist.
- `YOI_E2E_BIN=/home/hare/Projects/yoi/.worktree/e2e-binary-provider/target/debug/yoi OPENAI_API_KEY=host-secret ANTHROPIC_API_KEY=host-secret GEMINI_API_KEY=host-secret cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed; override provider path used without invoking the default cargo-build provider, and tested `yoi` subprocesses still used isolated env policy.

Remaining gaps:
- None known.
